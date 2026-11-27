'use strict';

// cedar-chat's Today page: the river. One day of what Cedar did (earlier), is doing and waits on
// you for (now), and will do (ahead), from GET /api/timeline; one box to ask with a When, sent
// with POST /api/asks. Plain JavaScript, no build step. Talks only to /api on this origin.
//
// What needs you is answered in place: a risky call is allowed or denied
// (POST /api/turns/{id}/approvals/{approval_id}), a question is answered with a message to its
// conversation, a response still going can be stopped. A plan to agree to opens the conversation,
// where its steps can be edited. A finished ask opens as its conversation on the main page (/v3/).

const $ = (id) => document.getElementById(id);
const TOKEN_KEY = 'cedar-chat-token';
/** How often the river refreshes while the page is in view. */
const REFRESH_MS = 60000;
/** How long after a notice from the server the page reloads: several often come together. */
const NOTICE_SETTLE_MS = 400;

const state = { day: null, data: null, timer: null, sending: false, drafted: null, draftNote: '' };

function storage(op, key, value) {
  try {
    if (op === 'get') return localStorage.getItem(key);
    if (op === 'set') localStorage.setItem(key, value);
  } catch { /* storage unavailable: the token lasts for this page only */ }
  return null;
}

let memoryToken = storage('get', TOKEN_KEY);
const zone = (() => { try { return Intl.DateTimeFormat().resolvedOptions().timeZone || 'UTC'; } catch { return 'UTC'; } })();
const locale = cedarI18n.locale;

// -- API ---------------------------------------------------------------------------------------

let tokenPrompt = null;
function askToken(rejected) {
  if (!tokenPrompt) {
    tokenPrompt = new Promise((resolve) => {
      $('token-message').textContent = tr(rejected ? 'today.tokenRejected' : 'today.tokenRequired');
      $('token-input').value = '';
      $('token-form').onsubmit = () => {
        memoryToken = $('token-input').value.trim();
        storage('set', TOKEN_KEY, memoryToken);
        tokenPrompt = null;
        resolve();
      };
      $('token-dialog').addEventListener('cancel', (e) => e.preventDefault(), { once: true });
      $('token-dialog').showModal();
    });
  }
  return tokenPrompt;
}

async function api(method, path, body) {
  const headers = memoryToken ? { Authorization: `Bearer ${memoryToken}` } : {};
  if (body !== undefined) headers['Content-Type'] = 'application/json';
  const res = await fetch(`/api${path}`, { method, headers, body: body === undefined ? undefined : JSON.stringify(body) });
  if (res.status === 401) { await askToken(Boolean(memoryToken)); return api(method, path, body); }
  if (res.status === 204) return null;
  const data = await res.json().catch(() => null);
  if (!res.ok) throw new Error(data?.error?.message || tr('today.requestFailed', { status: String(res.status) }));
  return data;
}

// -- formatting --------------------------------------------------------------------------------

const fmtTime = (iso) => new Date(iso).toLocaleTimeString(locale, { hour: '2-digit', minute: '2-digit' });
const localDay = (date) => `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, '0')}-${String(date.getDate()).padStart(2, '0')}`;
const parseDay = (day) => { const [y, m, d] = day.split('-').map(Number); return new Date(y, m - 1, d); };

/** A clock time "07:00" in the user's way of writing times. */
function fmtClock(hhmm) {
  const [h, m] = String(hhmm || '').split(':').map(Number);
  if (Number.isNaN(h)) return hhmm || '';
  const d = new Date(); d.setHours(h, m || 0, 0, 0);
  return d.toLocaleTimeString(locale, { hour: '2-digit', minute: '2-digit' });
}

/** When an agent is next due: its time on the day shown, else the weekday and time. */
function fmtDue(iso) {
  const at = new Date(iso);
  if (localDay(at) === state.data?.day) return fmtTime(iso);
  return at.toLocaleString(locale, { weekday: 'short', hour: '2-digit', minute: '2-digit' });
}

function weekdayName(index, style = 'long') {
  // 2024-01-01 was a Monday, and index 0 is Monday.
  return new Date(2024, 0, 1 + index).toLocaleDateString(locale, { weekday: style });
}

/** What starts an agent's runs, in a few words. */
function describe(trigger, kind) {
  const t = trigger || { kind };
  switch (t.kind) {
    case 'time':
      if (t.frequency === 'weekdays') return tr('today.desc.weekdays', { time: fmtClock(t.time) });
      if (t.frequency === 'weekly') return tr('today.desc.weekly', { day: weekdayName(t.weekday ?? 0), time: fmtClock(t.time) });
      return tr('today.desc.daily', { time: fmtClock(t.time) });
    case 'once': {
      const at = t.at ? new Date(t.at) : null;
      return at && !Number.isNaN(at.getTime())
        ? tr('today.desc.onceAt', { when: at.toLocaleString(locale, { weekday: 'short', day: 'numeric', month: 'short', hour: '2-digit', minute: '2-digit' }) })
        : tr('today.desc.once');
    }
    case 'interval': return tr('today.desc.interval', { count: t.every_minutes });
    case 'monthly': return tr('today.desc.monthly', { day: String(t.day), time: fmtClock(t.time) });
    case 'email': return tr('today.desc.email');
    case 'calendar': return tr('today.desc.calendar');
    case 'webhook': case 'device': return tr('today.desc.call');
    default: return tr('today.desc.change');
  }
}

/** What a tool is, in a word or two; a tool this page doesn't know keeps its own name. */
const TOOL_KEYS = {
  web_search: 'webSearch', fetch_url: 'page', web_fetch: 'page', bash: 'command', run_python: 'code',
  read_file: 'file', write_file: 'file', read_document: 'file', download_file: 'file',
  generate_pdf: 'pdf', generate_chart: 'chart', remember: 'memory', recall: 'memory', ask_user: 'question',
  follow_up: 'followUp', suggest_follow_up: 'followUp', computer: 'computer', browser: 'browser',
  search_flights: 'flights', search_products: 'products', weather: 'weather', weather_forecast: 'weather',
  stock: 'stocks', stocks: 'stocks', compare_stocks: 'stocks', plan_trip: 'trip', show_map: 'map', show_recipe: 'recipe',
};
const TOOL_PRODUCTS = { gmail: 'Gmail', calendar: 'Google Calendar', drive: 'Google Drive', sheets: 'Google Sheets', slides: 'Google Slides', slack: 'Slack' };
function toolLabel(name) {
  const tool = String(name || '');
  if (TOOL_KEYS[tool]) return tr(`today.tool.${TOOL_KEYS[tool]}`);
  const family = tool.split('_')[0];
  if (TOOL_PRODUCTS[family]) return TOOL_PRODUCTS[family];
  if (family === 'records') return tr('today.tool.records');
  return tool.replace(/_/g, ' ');
}

// -- DOM helpers -------------------------------------------------------------------------------

function el(tag, className, text) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined && text !== null) node.textContent = text;
  return node;
}

function link(href, className, text) {
  const a = el('a', className, text);
  a.href = href;
  return a;
}

function button(text, className, onClick) {
  const b = el('button', className, text);
  b.type = 'button';
  b.addEventListener('click', onClick);
  return b;
}

const conversationHref = (id) => `/v3/#/c/${encodeURIComponent(id)}`;
const agentHref = (id) => `/v3/#/agents/${encodeURIComponent(id)}`;
const outcomeHref = (id) => `#/t/${encodeURIComponent(id)}`;

/** Run `work` with `node` disabled, showing a failure under `where`. */
async function busy(node, where, work) {
  node.disabled = true;
  try {
    await work();
    await load();
  } catch (e) {
    node.disabled = false;
    const error = el('p', 'error', e.message);
    error.setAttribute('role', 'alert');
    where.querySelector(':scope > .error')?.remove();
    where.append(error);
  }
}

// -- the river ---------------------------------------------------------------------------------

/** A turn as a row under earlier: when, what, how it went. */
function turnRow(t) {
  const row = el('li', 'row');
  row.append(el('div', 'time', fmtTime(t.created_at)));
  const what = el('div', 'what');
  const head = el('div', 'what-head');
  head.append(link(outcomeHref(t.turn_id), 'what-title', t.title || t.prompt));
  if (t.agent) {
    // A run titled like its agent says when the agent runs; another names the agent.
    const agent = state.data.standing.find((a) => a.id === t.agent.id);
    const tag = t.agent.title !== t.title ? t.agent.title : agent ? describe(agent.trigger, agent.trigger_kind) : '';
    if (tag) head.append(el('span', 'tag', tag));
  }
  if (t.state === 'failed') head.append(el('span', 'tag failed', tr('today.failed')));
  if (t.state === 'stopped') head.append(el('span', 'tag', tr('today.stopped')));
  what.append(head);
  const line = t.state === 'failed' ? t.error : t.summary;
  if (line) what.append(el('div', 'summary', line));
  row.append(what);
  return row;
}

/** A turn under now: going, waiting on a go-ahead or an OK, or asking a question. */
function nowCard(t) {
  const card = el('li', `card ${t.state}`);
  card.append(el('div', 'time', fmtTime(t.created_at)));
  const body = el('div', 'card-body');
  const head = el('div', 'card-head');
  head.append(link(outcomeHref(t.turn_id), 'card-title', t.title || t.prompt));
  const pill = { waiting: 'today.pill.waiting', asking: 'today.pill.asking' }[t.state] || 'today.pill.working';
  head.append(el('span', 'pill', tr(pill)));
  body.append(head);

  if (t.state === 'waiting') {
    for (const wait of t.waits) {
      if (wait.kind === 'plan') {
        body.append(el('p', '', tr('today.planWaiting')));
        const steps = Array.isArray(wait.steps) ? wait.steps.filter((s) => s && typeof s.title === 'string') : [];
        const actions = el('div', 'actions');
        if (steps.length) {
          // The plan as cedar wrote it: the go-ahead agrees to exactly these steps.
          const list = el('ol', 'plan');
          for (const step of steps) list.append(el('li', '', step.title));
          body.append(list);
          actions.append(
            button(tr('today.goAhead'), 'go', (e) => busy(e.currentTarget, body, () => api('POST', `/turns/${t.turn_id}/plan`, { steps }))),
            link(conversationHref(t.conversation_id), '', tr('today.changePlan')),
          );
        } else {
          actions.append(link(conversationHref(t.conversation_id), 'go', tr('today.reviewPlan')));
        }
        body.append(actions);
      } else {
        const approval = wait.key.replace(/^approval:/, '');
        body.append(el('p', '', wait.reason || tr('today.approvalWaiting')));
        const actions = el('div', 'actions');
        const decide = (decision) => (e) => busy(e.currentTarget, body, () =>
          api('POST', `/turns/${t.turn_id}/approvals/${encodeURIComponent(approval)}`, { decision }));
        actions.append(button(tr('today.allow'), 'go', decide('allow')), button(tr('today.deny'), '', decide('deny')));
        body.append(actions);
      }
    }
  } else if (t.state === 'asking') {
    if (t.summary) body.append(el('p', '', t.summary));
    const form = el('form', 'answer');
    const input = el('input');
    input.type = 'text';
    input.required = true;
    input.maxLength = 32000;
    input.setAttribute('aria-label', tr('today.answerLabel'));
    input.placeholder = tr('today.answerPlaceholder');
    const send = el('button', 'primary', tr('today.answer'));
    send.type = 'submit';
    form.append(input, send);
    form.addEventListener('submit', (e) => {
      e.preventDefault();
      const content = input.value.trim();
      if (!content) return;
      busy(send, body, () => api('POST', `/conversations/${t.conversation_id}/messages`, { content, locale, timezone: zone }));
    });
    body.append(form);
  } else {
    const actions = el('div', 'actions');
    actions.append(el('span', 'progress', progressText(t.progress)));
    actions.append(button(tr('today.stop'), '', (e) => busy(e.currentTarget, body, () => api('POST', `/turns/${t.turn_id}/cancel`))));
    body.append(actions);
  }
  card.append(body);
  return card;
}

/** Where a response still going is: what it is using now and how many steps so far. */
const progressText = (progress) => (progress?.steps
  ? `${toolLabel(progress.doing)} · ${tr('today.stepsSoFar', { count: progress.steps })}`
  : tr('today.working'));

/** A call an unattended run held for the person's OK (the outbox), under now: where it came from,
 *  and the shared card that sends, edits or discards it. */
function heldCard(item) {
  const card = el('li', 'card waiting held');
  card.append(el('div', 'time', fmtTime(item.created_at)));
  const body = el('div', 'card-body');
  const head = el('div', 'card-head');
  const name = item.agent?.title || item.title || tr('today.held.untitled');
  head.append(item.turn_id ? link(outcomeHref(item.turn_id), 'card-title', name) : el('span', 'card-title', name));
  head.append(el('span', 'pill', tr('today.pill.held')));
  body.append(head, heldElement(item, () => load()));
  card.append(body);
  return card;
}

/** The shared outbox card for a held call (handover-view.js); `onChange` after a decision. */
const heldElement = (item, onChange) =>
  cedarHandover.held.element({ id: item.id, tool: item.tool, reason: item.reason, why: item.why }, { item, onChange });

/** An agent under ahead: when it runs next (or "any time" for one that watches), and what. */
function aheadRow(a) {
  const row = el('li', 'row');
  row.append(el('div', 'time', a.watches ? tr('today.anyTime') : fmtDue(a.next_run_at)));
  const what = el('div', 'what');
  const head = el('div', 'what-head');
  head.append(link(agentHref(a.id), 'what-title', a.title));
  let tag = describe(a.trigger, a.trigger_kind);
  if (a.watches && a.last_checked_at) tag = tr('today.lastChecked', { what: tag, time: fmtTime(a.last_checked_at) });
  head.append(el('span', 'tag', tag));
  what.append(head);
  row.append(what);
  return row;
}

function standingItem(a) {
  const li = el('li');
  const name = el('div', 'name');
  name.append(link(agentHref(a.id), '', a.title), el('span', 'tag', describe(a.trigger, a.trigger_kind)));
  const label = !a.enabled ? 'today.state.paused' : a.watches ? 'today.state.watching' : 'today.state.on';
  li.append(name, el('span', `state${a.enabled ? '' : ' off'}`, tr(label)));
  return li;
}

function render() {
  const d = state.data;
  const date = parseDay(d.day);
  $('day-name').textContent = date.toLocaleDateString(locale, { weekday: 'long', day: 'numeric', month: 'long' });
  document.title = tr('today.pageTitle', { day: $('day-name').textContent });
  $('to-today').hidden = d.today;
  $('next-day').hidden = false;

  $('needs').hidden = !d.needs_you;
  $('needs-text').textContent = tr('today.needs', { count: d.needs_you });

  $('earlier').replaceChildren(...d.earlier.map(turnRow));
  $('earlier-empty').hidden = d.earlier.length > 0;
  $('earlier-empty').textContent = tr(d.today ? 'today.earlierEmpty' : 'today.dayEmpty');

  // Now and the box to ask are today's.
  $('now').hidden = !d.today;
  $('now-time').textContent = fmtTime(new Date().toISOString());
  // What waits on the person, turns and held calls alike, in the order it came.
  const now = [
    ...d.now.map((t) => ({ at: t.created_at, node: nowCard(t) })),
    ...(d.outbox || []).map((item) => ({ at: item.created_at, node: heldCard(item) })),
  ].sort((a, b) => String(a.at).localeCompare(String(b.at)));
  $('now-list').replaceChildren(...now.map((n) => n.node));

  const ahead = [...d.ahead, ...d.watching];
  $('ahead').replaceChildren(...ahead.map(aheadRow));
  $('ahead-empty').hidden = ahead.length > 0;
  $('ahead-section').hidden = !ahead.length && !d.today;

  $('standing').replaceChildren(...d.standing.map(standingItem));
  $('standing-empty').hidden = d.standing.length > 0;
}

async function load() {
  const query = new URLSearchParams({ timezone: zone });
  if (state.day) query.set('day', state.day);
  try {
    state.data = await api('GET', `/timeline?${query}`);
    render();
  } catch (e) {
    $('earlier-empty').hidden = false;
    $('earlier-empty').textContent = e.message;
  }
}

function go(day) {
  const today = localDay(new Date());
  state.day = !day || day === today ? null : day;
  const hash = state.day ? `#${state.day}` : '';
  if (location.hash !== hash) history.replaceState(null, '', `${location.pathname}${hash}`);
  load();
}

function shift(days) {
  const date = parseDay(state.data?.day || localDay(new Date()));
  date.setDate(date.getDate() + days);
  go(localDay(date));
}

// -- asking ------------------------------------------------------------------------------------

const SEND_LABELS = { now: 'today.send.now', later: 'today.send.later', every: 'today.send.every', watch: 'today.send.watch' };
const HINTS = { now: 'today.hint.now', later: 'today.hint.later', every: 'today.hint.every', watch: 'today.hint.watch' };

const currentWhen = () => document.querySelector('input[name=when]:checked')?.value || 'now';

function whenChanged(event) {
  // A choice made by hand replaces what was read out of the sentence.
  if (event) { state.drafted = null; state.draftNote = ''; }
  const when = currentWhen();
  for (const detail of document.querySelectorAll('.when-detail')) detail.hidden = detail.dataset.for !== when;
  const privateBox = $('private');
  if (when !== 'now') privateBox.checked = false;
  $('private-wrap').hidden = when !== 'now';
  $('ask-send').textContent = tr(SEND_LABELS[when]);
  $('when-hint').textContent = state.draftNote || tr(privateBox.checked ? 'today.hint.private' : HINTS[when]);
  $('every-weekday-wrap').hidden = $('every-frequency').value !== 'weekly';
  const kind = $('watch-kind').value;
  for (const field of document.querySelectorAll('[data-watch]')) field.hidden = field.dataset.watch !== kind;
}

/** The trigger the form describes for a When other than now. */
function triggerFor(when) {
  // A reading the form can't show (monthly, every N minutes) is sent as it was read.
  if (state.drafted?.when === when) return state.drafted.trigger;
  if (when === 'later') {
    const at = $('later-at').value;
    if (!at) throw new Error(tr('today.later.missing'));
    return { kind: 'once', at: at.slice(0, 16), timezone: zone };
  }
  if (when === 'every') {
    const frequency = $('every-frequency').value;
    const trigger = { kind: 'time', frequency, time: $('every-time').value || '07:00', timezone: zone };
    if (frequency === 'weekly') trigger.weekday = Number($('every-weekday').value);
    return trigger;
  }
  const kind = $('watch-kind').value;
  if (kind === 'email') {
    const query = $('watch-query').value.trim();
    if (!query) throw new Error(tr('today.watch.queryMissing'));
    return { kind: 'email', query };
  }
  if (kind === 'calendar') return { kind: 'calendar', minutes_before: Number($('watch-minutes').value) || 30 };
  return { kind: 'device' };
}

async function ask(e) {
  e.preventDefault();
  if (state.sending) return;
  const content = $('ask-input').value.trim();
  const error = $('ask-error');
  error.hidden = true;
  if (!content) { $('ask-input').focus(); return; }
  const when = currentWhen();
  let body;
  try {
    body = when === 'now'
      ? { content, private: $('private').checked, locale, timezone: zone }
      : { content, when, trigger: triggerFor(when), timezone: zone };
  } catch (err) {
    error.textContent = err.message;
    error.hidden = false;
    return;
  }
  state.sending = true;
  $('ask-send').disabled = true;
  try {
    const made = await api('POST', '/asks', body);
    $('ask-input').value = '';
    state.drafted = null;
    state.draftNote = '';
    whenChanged();
    // Off the record stays out of the river: it lives only in its conversation.
    if (made.conversation?.private) { location.href = conversationHref(made.conversation.id); return; }
    await load();
  } catch (err) {
    error.textContent = err.message;
    error.hidden = false;
  } finally {
    state.sending = false;
    $('ask-send').disabled = false;
  }
}

/** Later defaults to the next full hour, two hours from now. */
function defaultLater() {
  const at = new Date(Date.now() + 2 * 3600 * 1000);
  at.setMinutes(0, 0, 0);
  return `${localDay(at)}T${String(at.getHours()).padStart(2, '0')}:00`;
}

// -- one ask's outcome -------------------------------------------------------------------------

let toastTimer = null;
function toast(message) {
  const box = $('toast');
  box.textContent = message;
  box.hidden = false;
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => { box.hidden = true; }, 5000);
}

/** Markdown as DOM: headings, lists, quotes, code, tables and inline marks, every piece of text
 *  set as text. Links go only to http(s); `[n]` citations link to the run's nth source. */
function renderMarkdown(text, sources = []) {
  const root = document.createDocumentFragment();
  const lines = String(text || '').replace(/\r/g, '').split('\n');
  const item = /^\s*([-*+]|\d+[.)])\s+/;
  const block = (line) => /^\s*(```|#{1,6}\s|>|\|)/.test(line) || item.test(line);
  let i = 0;
  while (i < lines.length) {
    const line = lines[i];
    let m;
    if (/^\s*```/.test(line)) {
      const code = [];
      for (i++; i < lines.length && !/^\s*```/.test(lines[i]); i++) code.push(lines[i]);
      i++;
      const pre = el('pre');
      pre.append(el('code', '', code.join('\n')));
      root.append(pre);
    } else if ((m = line.match(/^(#{1,6})\s+(.*)$/))) {
      // The page's own heading is the outcome; the answer's start a level below it.
      const h = el(`h${Math.min(6, m[1].length + 1)}`);
      inline(h, m[2], sources);
      root.append(h);
      i++;
    } else if (item.test(line)) {
      const list = el(/^\s*\d/.test(line) ? 'ol' : 'ul');
      for (; i < lines.length && item.test(lines[i]); i++) {
        const li = el('li');
        inline(li, lines[i].replace(item, ''), sources);
        list.append(li);
      }
      root.append(list);
    } else if (/^\s*>/.test(line)) {
      const quoted = [];
      for (; i < lines.length && /^\s*>/.test(lines[i]); i++) quoted.push(lines[i].replace(/^\s*> ?/, ''));
      const q = el('blockquote');
      q.append(renderMarkdown(quoted.join('\n'), sources));
      root.append(q);
    } else if (/^\s*\|/.test(line)) {
      const rows = [];
      for (; i < lines.length && /^\s*\|/.test(lines[i]); i++) rows.push(lines[i]);
      const cells = (row) => row.trim().replace(/^\||\|$/g, '').split('|').map((c) => c.trim());
      const wrap = el('div', 'table-wrap');
      const table = el('table');
      rows.filter((r) => !/^\s*\|[\s:|-]+\|\s*$/.test(r)).forEach((row, n) => {
        const tr_ = el('tr');
        for (const c of cells(row)) { const cell = el(n === 0 ? 'th' : 'td'); inline(cell, c, sources); tr_.append(cell); }
        table.append(tr_);
      });
      wrap.append(table);
      root.append(wrap);
    } else if (!line.trim()) {
      i++;
    } else {
      const para = [];
      for (; i < lines.length && lines[i].trim() && !block(lines[i]); i++) para.push(lines[i].trim());
      const p = el('p');
      inline(p, para.join(' '), sources);
      root.append(p);
    }
  }
  return root;
}

function inline(parent, text, sources) {
  const token = /(\*\*[^*]+\*\*|`[^`]+`|\[[^\]]+\]\([^)\s]+\)|\[\d+\]|\*[^*\s][^*]*\*|_[^_\s][^_]*_)/;
  for (const part of String(text).split(token)) {
    if (!part) continue;
    let m;
    if (/^\*\*.+\*\*$/.test(part)) parent.append(el('strong', '', part.slice(2, -2)));
    else if (/^`.+`$/.test(part)) parent.append(el('code', '', part.slice(1, -1)));
    else if ((m = part.match(/^\[([^\]]+)\]\(([^)\s]+)\)$/))) {
      if (/^https?:\/\//i.test(m[2])) {
        const a = link(m[2], '', m[1]);
        a.rel = 'noopener noreferrer';
        a.target = '_blank';
        parent.append(a);
      } else parent.append(document.createTextNode(m[1]));
    } else if ((m = part.match(/^\[(\d+)\]$/))) {
      const source = sources[Number(m[1]) - 1];
      if (source?.url && /^https?:\/\//i.test(source.url)) {
        const sup = el('sup');
        const a = link(source.url, '', m[1]);
        a.rel = 'noopener noreferrer';
        a.target = '_blank';
        a.title = source.title || source.url;
        sup.append(a);
        parent.append(sup);
      } else parent.append(document.createTextNode(part));
    } else if (/^([*_]).+\1$/.test(part)) parent.append(el('em', '', part.slice(1, -1)));
    else parent.append(document.createTextNode(part));
  }
}

const fmtSize = (bytes) => bytes < 1024 ? tr('today.outcome.bytes', { size: cedarI18n.number(bytes) })
  : bytes < 1024 * 1024 ? tr('today.outcome.kb', { size: cedarI18n.number(Math.round(bytes / 1024)) })
  : tr('today.outcome.mb', { size: cedarI18n.number(bytes / 1048576, { maximumFractionDigits: 1 }) });

/** Download a workspace file with the access token, as the main page does. */
async function download(conversationId, path, node) {
  node.disabled = true;
  try {
    const url = `/api/conversations/${conversationId}/files/${path.split('/').map(encodeURIComponent).join('/')}`;
    let res = await fetch(url, { headers: memoryToken ? { Authorization: `Bearer ${memoryToken}` } : {} });
    if (res.status === 401) { await askToken(true); res = await fetch(url, { headers: { Authorization: `Bearer ${memoryToken}` } }); }
    if (!res.ok) throw new Error(tr('today.requestFailed', { status: String(res.status) }));
    const href = URL.createObjectURL(await res.blob());
    const a = link(href, '', '');
    a.download = path.split('/').pop();
    document.body.append(a);
    a.click();
    a.remove();
    setTimeout(() => URL.revokeObjectURL(href), 60000);
  } catch (e) { toast(e.message); } finally { node.disabled = false; }
}

/** A titled part of the outcome; nothing when it would be empty. */
function part(title, children) {
  if (!children.length) return null;
  const section = el('section', 'part');
  section.append(el('h2', '', title), ...children);
  return section;
}

const outcomeState = { id: null, timer: null };

function hideOutcome() {
  outcomeState.id = null;
  clearTimeout(outcomeState.timer);
  $('outcome').hidden = true;
  $('layout').hidden = false;
  $('day-nav').hidden = false;
}

async function showOutcome(id) {
  const first = outcomeState.id !== id;
  outcomeState.id = id;
  clearTimeout(outcomeState.timer);
  $('layout').hidden = true;
  $('day-nav').hidden = true;
  $('search').hidden = true;
  $('outcome').hidden = false;
  if (first) { $('outcome').replaceChildren(el('p', 'quiet', tr('today.outcome.loading'))); window.scrollTo(0, 0); }
  let o;
  try {
    o = await api('GET', `/turns/${encodeURIComponent(id)}/outcome`);
  } catch (e) {
    if (outcomeState.id === id) $('outcome').replaceChildren(backLink(null), el('p', 'error', e.message));
    return;
  }
  if (outcomeState.id !== id) return;
  // Not while something is being typed into it: rendering would replace the field.
  if (!first && $('outcome').contains(document.activeElement) && document.activeElement.matches('input, textarea')) return;
  renderOutcome(o);
  document.title = tr('today.pageTitle', { day: o.conversation.title || o.turn.prompt });
  // A response still going is followed until it ends.
  if (o.state === 'working') outcomeState.timer = setTimeout(() => showOutcome(id), 3000);
}

/** Back to the river of the day the ask was made. */
function backLink(turn) {
  const day = turn ? localDay(new Date(turn.created_at)) : null;
  const a = link(!day || day === localDay(new Date()) ? '#' : `#${day}`, 'back', '');
  a.append(el('span', '', tr('today.outcome.back')));
  return a;
}

function renderOutcome(o) {
  const t = o.turn;
  const head = el('header', 'outcome-head');
  const meta = el('div', 'outcome-meta');
  meta.append(backLink(t));
  const when = {
    done: () => tr('today.outcome.doneAt', { time: fmtTime(t.finished_at || t.created_at) }),
    failed: () => tr('today.outcome.failedAt', { time: fmtTime(t.finished_at || t.created_at) }),
    stopped: () => tr('today.outcome.stopped'),
    asking: () => tr('today.pill.asking'),
    working: () => tr('today.working'),
  }[o.state]();
  meta.append(el('span', `tag ${o.state === 'failed' ? 'failed' : ''}`, o.agent ? `${when} · ${o.agent.title}` : when));
  head.append(meta);
  head.append(el('p', 'asked', tr('today.outcome.youAsked', { prompt: t.prompt })));
  const headline = o.state === 'failed' ? (t.error || tr('today.failed')) : (o.summary || o.conversation.title || t.prompt);
  head.append(el('h1', '', headline));
  if (o.later_turns) {
    const more = link(conversationHref(o.conversation.id), 'tag', tr('today.outcome.continued', { count: o.later_turns }));
    head.append(more);
  }

  const body = el('div', 'outcome-body');
  if (o.state === 'working') {
    const actions = el('div', 'actions');
    actions.append(el('span', 'progress', tr('today.working')));
    actions.append(button(tr('today.stop'), '', (e) => busyOutcome(e.currentTarget, () => api('POST', `/turns/${t.id}/cancel`))));
    body.append(actions);
  }
  if (t.final_text) {
    const answer = el('div', 'answer-text');
    answer.append(renderMarkdown(t.final_text, o.sources));
    // The headline is the answer's first paragraph: it isn't said twice.
    const first = answer.querySelector('p');
    if (first && o.summary) {
      const plain = first.cloneNode(true);
      plain.querySelectorAll('sup').forEach((n) => n.remove());
      const words = (x) => x.replace(/\s+([.,;:!?])/g, '$1').replace(/\s+/g, ' ').trim();
      if (words(plain.textContent) === words(o.summary)) {
        // A heading left with nothing under it goes too.
        const [before, after] = [first.previousElementSibling, first.nextElementSibling];
        first.remove();
        if (before && /^H\d$/.test(before.tagName) && (!after || /^H\d$/.test(after.tagName))) before.remove();
      }
    }
    if (answer.childNodes.length) body.append(answer);
  }

  // What it left behind.
  const files = o.files.map((f) => {
    const b = button('', 'file', (e) => download(o.conversation.id, f.path, e.currentTarget));
    b.append(el('span', 'file-name', f.path.split('/').pop()), el('span', 'tag', `${f.path} · ${fmtSize(f.size || 0)}`));
    return b;
  });
  const memories = o.memories.map((m) => {
    const row = el('div', 'kept');
    row.append(el('span', '', m.text));
    const forget = button(tr('today.outcome.forget'), 'linkish', async (e) => {
      e.currentTarget.disabled = true;
      try {
        await api('DELETE', `/memory/${encodeURIComponent(m.id)}`);
        row.replaceChildren(el('span', 'quiet', tr('today.outcome.forgotten', { text: m.text })));
      } catch (err) { e.currentTarget.disabled = false; toast(err.message); }
    });
    row.append(forget);
    return row;
  });
  const records = o.records.map((r) => {
    const row = el('div', 'kept');
    const text = el('span', 'record');
    text.append(el('span', '', tr('today.outcome.saved', { set: r.set, key: r.key || '' })));
    // The row's values, briefly: text and numbers only.
    const values = Object.entries(r.data && typeof r.data === 'object' ? r.data : {})
      .filter(([, v]) => ['string', 'number', 'boolean'].includes(typeof v) && String(v) !== '')
      .slice(0, 6).map(([k, v]) => `${k}: ${String(v).slice(0, 80)}`);
    if (values.length) text.append(el('span', 'tag', values.join(' · ')));
    row.append(text);
    return row;
  });
  cedarFollowUp.known(o.followup_decisions);
  const followups = o.followups.map((f) => {
    const spec = cedarFollowUp.parse(String(f.output ?? ''));
    return spec && cedarFollowUp.element(spec, { turnId: t.id, callId: f.call_id });
  }).filter(Boolean);

  // What still waits for an OK comes first, and heads the part; settled calls say what became of them.
  const heldItems = [...(o.held || [])].sort((a, b) => (a.status === 'pending' ? 0 : 1) - (b.status === 'pending' ? 0 : 1));
  const held = heldItems.map((item) => heldElement(item, () => showOutcome(t.id)));
  const waiting = heldItems.some((item) => item.status === 'pending');
  const parts = [
    part(tr(waiting ? 'today.outcome.held' : 'today.outcome.heldDone'), held),
    part(tr('today.outcome.files'), files),
    part(tr('today.outcome.remembered'), memories),
    part(tr('today.outcome.records'), records),
    part(tr('followup.suggests'), followups),
  ].filter(Boolean);

  // How it got there, folded.
  if (o.steps.length || o.sources.length) {
    const how = el('details', 'how');
    const failed = o.steps.filter((s) => s.is_error).length;
    const counts = [tr('today.outcome.steps', { count: o.steps.length })];
    if (o.sources.length) counts.push(tr('today.outcome.sources', { count: o.sources.length }));
    if (failed) counts.push(tr('today.outcome.stepsFailed', { count: failed }));
    how.append(el('summary', '', `${tr('today.outcome.how')} · ${counts.join(' · ')}`));
    if (o.steps.length) {
      const list = el('ol', 'steps');
      for (const s of o.steps) {
        const li = el('li', s.is_error ? 'failed' : '');
        const what = el('span', '', toolLabel(s.name));
        what.title = s.name;
        li.append(what);
        if (s.is_error) li.append(el('span', 'tag failed', tr('today.outcome.stepFailed')));
        list.append(li);
      }
      how.append(list);
    }
    if (o.sources.length) {
      const list = el('ol', 'sources');
      for (const s of o.sources) {
        const li = el('li');
        if (s.url && /^https?:\/\//i.test(s.url)) {
          const a = link(s.url, '', s.title || s.url);
          a.rel = 'noopener noreferrer';
          a.target = '_blank';
          li.append(a);
        } else li.append(el('span', '', s.title || s.path || s.url || ''));
        list.append(li);
      }
      how.append(list);
    }
    parts.push(how);
  }

  // Keep going: the next message of the same conversation, or the answer to its question.
  const form = el('form', 'keep-going');
  const input = el('input');
  input.type = 'text';
  input.required = true;
  input.maxLength = 32000;
  const label = o.state === 'asking' ? tr('today.answerPlaceholder') : tr('today.outcome.keepGoing');
  input.placeholder = label;
  input.setAttribute('aria-label', label);
  const send = el('button', 'primary', o.state === 'asking' ? tr('today.answer') : tr('today.send.now'));
  send.type = 'submit';
  send.disabled = o.state === 'working';
  form.append(input, send);
  form.addEventListener('submit', (e) => {
    e.preventDefault();
    const content = input.value.trim();
    if (!content) return;
    busyOutcome(send, async () => {
      const next = await api('POST', `/conversations/${o.conversation.id}/messages`, { content, locale, timezone: zone });
      location.hash = outcomeHref(next.id);
    });
  });
  const foot = el('footer', 'outcome-foot');
  foot.append(form, link(conversationHref(o.conversation.id), 'linkish', tr('today.outcome.open')));

  $('outcome').replaceChildren(head, body, ...parts, foot);
}

async function busyOutcome(node, work) {
  node.disabled = true;
  try {
    await work();
    if (outcomeState.id) await showOutcome(outcomeState.id);
  } catch (e) {
    node.disabled = false;
    toast(e.message);
  }
}

// -- reading the When, searching, and hearing from the server -----------------------------------

/** Ask the server what When the sentence asks for, and set the form to it; nothing is sent. */
async function draftWhen() {
  const content = $('ask-input').value.trim();
  const error = $('ask-error');
  error.hidden = true;
  if (!content) { $('ask-input').focus(); return; }
  const node = $('when-draft');
  node.disabled = true;
  node.textContent = tr('today.draft.working');
  try {
    applyDraft(await api('POST', '/asks/draft', { content, timezone: zone }));
  } catch (e) {
    error.textContent = e.message;
    error.hidden = false;
  } finally {
    node.disabled = false;
    node.textContent = tr('today.draft.button');
  }
}

/** Put a reading (`{when, trigger, content}`) into the form, for the person to check and send. */
function applyDraft(d) {
  const when = ['now', 'later', 'every', 'watch'].includes(d.when) ? d.when : 'now';
  const t = d.trigger || {};
  if (typeof d.content === 'string' && d.content.trim()) $('ask-input').value = d.content;
  document.querySelector(`input[name=when][value=${when}]`).checked = true;
  state.drafted = null;
  if (t.kind === 'once' && t.at) $('later-at').value = String(t.at).slice(0, 16);
  else if (t.kind === 'time') {
    $('every-frequency').value = ['daily', 'weekdays', 'weekly'].includes(t.frequency) ? t.frequency : 'daily';
    if (t.weekday != null) $('every-weekday').value = String(t.weekday);
    if (t.time) $('every-time').value = t.time;
  } else if (t.kind === 'email') { $('watch-kind').value = 'email'; $('watch-query').value = t.query || ''; }
  else if (t.kind === 'calendar') { $('watch-kind').value = 'calendar'; $('watch-minutes').value = String(t.minutes_before || 30); }
  else if (t.kind) state.drafted = { when, trigger: t };
  state.draftNote = when === 'now' ? tr('today.draft.now') : tr('today.draft.read', { what: describe(t, t.kind) });
  whenChanged();
  // What the form can't show is said by the note alone: its own fields stay out of the way.
  if (state.drafted) for (const detail of document.querySelectorAll('.when-detail')) detail.hidden = true;
}

let searchToken = 0;
async function showSearch(q) {
  const token = ++searchToken;
  clearTimeout(outcomeState.timer);
  $('layout').hidden = true;
  $('day-nav').hidden = true;
  $('outcome').hidden = true;
  const view = $('search');
  view.hidden = false;
  $('search-input').value = q;
  const back = link('#', 'back', '');
  back.append(el('span', '', tr('today.outcome.back')));
  view.replaceChildren(back, el('p', 'quiet', tr('today.outcome.loading')));
  let found;
  try {
    found = await api('GET', `/search?q=${encodeURIComponent(q)}`);
  } catch (e) {
    if (token === searchToken) view.replaceChildren(back, el('p', 'error', e.message));
    return;
  }
  if (token !== searchToken) return;
  document.title = tr('today.pageTitle', { day: q });

  const conversations = found.conversations.map((c) => {
    const row = el('div', 'hit');
    const title = c.title || c.snippet || '';
    row.append(c.turn_id ? link(outcomeHref(c.turn_id), 'file-name', title) : link(conversationHref(c.id), 'file-name', title));
    if (c.snippet) row.append(el('span', 'summary', c.snippet));
    if (c.schedule_title) row.append(el('span', 'tag', c.schedule_title));
    return row;
  });
  const agents = found.agents.map((a) => {
    const row = el('div', 'hit');
    row.append(link(agentHref(a.id), 'file-name', a.title), el('span', 'tag', describe(a.trigger, a.trigger_kind)));
    return row;
  });
  const memories = found.memories.map((m) => {
    const row = el('div', 'kept');
    row.append(el('span', '', m.text));
    row.append(button(tr('today.outcome.forget'), 'linkish', async (e) => {
      e.currentTarget.disabled = true;
      try {
        await api('DELETE', `/memory/${encodeURIComponent(m.id)}`);
        row.replaceChildren(el('span', 'quiet', tr('today.outcome.forgotten', { text: m.text })));
      } catch (err) { e.currentTarget.disabled = false; toast(err.message); }
    }));
    return row;
  });
  const records = found.records.map((r) => {
    const row = el('div', 'hit');
    row.append(el('span', 'file-name', r.name));
    if (r.description) row.append(el('span', 'summary', r.description));
    row.append(el('span', 'tag', tr('today.search.rows', { count: Number(r.count) || 0 })));
    return row;
  });
  const parts = [
    part(tr('today.search.conversations'), conversations),
    part(tr('today.search.agents'), agents),
    part(tr('today.search.memories'), memories),
    part(tr('today.search.records'), records),
  ].filter(Boolean);
  const head = el('header', 'outcome-head');
  head.append(back, el('h1', '', tr('today.search.title', { q })));
  view.replaceChildren(head, ...(parts.length ? parts : [el('p', 'quiet', tr('today.search.none'))]));
}

/**
 * Hear from the server (GET /api/events: a response finished, one waits, the outbox changed) and
 * reload what is showing, so the page doesn't wait for its next minute. The stream has no replay,
 * and the minute's reload covers what it misses.
 */
async function listen() {
  let settle = null;
  const changed = () => {
    clearTimeout(settle);
    settle = setTimeout(() => {
      if (document.visibilityState !== 'visible') return;
      if (outcomeState.id) showOutcome(outcomeState.id);
      else if (!$('layout').hidden && !$('now-list').contains(document.activeElement)) load();
    }, NOTICE_SETTLE_MS);
  };
  const decoder = new TextDecoder();
  for (;;) {
    try {
      const res = await fetch('/api/events', { headers: memoryToken ? { Authorization: `Bearer ${memoryToken}` } : {} });
      if (res.ok && res.body) {
        const reader = res.body.getReader();
        for (;;) {
          const { done, value } = await reader.read();
          if (done) break;
          // Keep-alive comments are not news.
          if (/^data:/m.test(decoder.decode(value, { stream: true }))) changed();
        }
      }
    } catch { /* offline, or the server restarted: try again */ }
    await new Promise((resolve) => { setTimeout(resolve, 5000); });
  }
}

// -- start -------------------------------------------------------------------------------------

function refreshSoon() {
  clearInterval(state.timer);
  state.timer = setInterval(() => {
    // Not while an answer is being typed: rendering would replace the field.
    if (document.visibilityState !== 'visible' || $('layout').hidden || $('now-list').contains(document.activeElement)) return;
    load();
  }, REFRESH_MS);
}

function start() {
  const weekday = $('every-weekday');
  weekday.replaceChildren(...[0, 1, 2, 3, 4, 5, 6].map((i) => { const o = el('option', '', weekdayName(i)); o.value = String(i); return o; }));
  $('later-at').value = defaultLater();
  $('later-at').min = `${localDay(new Date())}T00:00`;

  for (const input of document.querySelectorAll('input[name=when]')) input.addEventListener('change', whenChanged);
  for (const id of ['every-frequency', 'watch-kind', 'private']) $(id).addEventListener('change', whenChanged);
  // A field changed by hand is no longer what was read out of the sentence.
  for (const id of ['later-at', 'every-time', 'every-weekday', 'watch-query', 'watch-minutes']) $(id).addEventListener('input', whenChanged);
  $('ask').addEventListener('submit', ask);
  $('ask-input').addEventListener('keydown', (e) => {
    if (e.key === 'Enter' && !e.shiftKey && !e.isComposing) { e.preventDefault(); $('ask').requestSubmit(); }
  });
  $('private-offer').addEventListener('click', () => {
    document.querySelector('input[name=when][value=now]').checked = true;
    $('private').checked = true;
    whenChanged();
    $('ask-input').focus();
  });
  $('prev-day').addEventListener('click', () => shift(-1));
  $('next-day').addEventListener('click', () => shift(1));
  $('to-today').addEventListener('click', () => go(null));
  document.addEventListener('visibilitychange', () => { if (document.visibilityState === 'visible' && !$('layout').hidden) load(); });
  window.addEventListener('hashchange', route);

  whenChanged();
  refreshSoon();
  $('when-draft').addEventListener('click', draftWhen);
  $('search-form').addEventListener('submit', (e) => {
    e.preventDefault();
    const q = $('search-input').value.trim();
    if (q) location.hash = `#/q/${encodeURIComponent(q)}`;
  });
  cedarFollowUp.init({ api, toast });
  listen();
  // A Cedar with no model yet is set up on the main page, which opens its setup by itself.
  api('GET', '/models').then((m) => { if (Array.isArray(m?.models) && !m.models.length) location.replace('/v3/'); }).catch(() => {});
  if (!route()) go(null);
}

/** Follow the address: `#/t/<turn>` is an outcome, a date is that day's river, `#/agents/…` (from a
 *  follow-up card) is on the main page, and other anchors (#now) only scroll. Says whether it
 *  showed something. */
function route() {
  const hash = location.hash.slice(1);
  // An address of the main page (a notification's #/c/<id>, #/agents/…, settings) is handed to it.
  if (hash.startsWith('/') && !/^\/(t|q)\//.test(hash)) { location.replace(`/v3/#${hash}`); return true; }
  const turn = hash.match(/^\/t\/([0-9a-fA-F-]{36})$/);
  if (turn) { $('search').hidden = true; showOutcome(turn[1]); return true; }
  const found = hash.match(/^\/q\/(.+)$/);
  if (found) { hideOutcome(); showSearch(decodeURIComponent(found[1])); return true; }
  const wasOutcome = !$('outcome').hidden || !$('search').hidden;
  $('search').hidden = true;
  hideOutcome();
  if (!hash || /^\d{4}-\d{2}-\d{2}$/.test(hash)) { go(hash || null); return true; }
  if (wasOutcome) load();
  return false;
}

start();

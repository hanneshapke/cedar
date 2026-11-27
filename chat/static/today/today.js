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
const REFRESH_MS = 15000;

const state = { day: null, data: null, timer: null, sending: false };

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
    case 'once': return tr('today.desc.once');
    case 'interval': return tr('today.desc.interval', { count: t.every_minutes });
    case 'monthly': return tr('today.desc.monthly', { day: String(t.day), time: fmtClock(t.time) });
    case 'email': return tr('today.desc.email');
    case 'calendar': return tr('today.desc.calendar');
    case 'webhook': case 'device': return tr('today.desc.call');
    default: return tr('today.desc.change');
  }
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
  head.append(link(conversationHref(t.conversation_id), 'what-title', t.title || t.prompt));
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
  head.append(link(conversationHref(t.conversation_id), 'card-title', t.title || t.prompt));
  const pill = { waiting: 'today.pill.waiting', asking: 'today.pill.asking' }[t.state] || 'today.pill.working';
  head.append(el('span', 'pill', tr(pill)));
  body.append(head);

  if (t.state === 'waiting') {
    for (const wait of t.waits) {
      if (wait.kind === 'plan') {
        body.append(el('p', '', tr('today.planWaiting')));
        const actions = el('div', 'actions');
        actions.append(link(conversationHref(t.conversation_id), 'go', tr('today.reviewPlan')));
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
    actions.append(el('span', 'progress', tr('today.working')));
    actions.append(button(tr('today.stop'), '', (e) => busy(e.currentTarget, body, () => api('POST', `/turns/${t.turn_id}/cancel`))));
    body.append(actions);
  }
  card.append(body);
  return card;
}

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
  $('now-list').replaceChildren(...d.now.map(nowCard));

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

function whenChanged() {
  const when = currentWhen();
  for (const detail of document.querySelectorAll('.when-detail')) detail.hidden = detail.dataset.for !== when;
  const privateBox = $('private');
  if (when !== 'now') privateBox.checked = false;
  $('private-wrap').hidden = when !== 'now';
  $('ask-send').textContent = tr(SEND_LABELS[when]);
  $('when-hint').textContent = tr(privateBox.checked ? 'today.hint.private' : HINTS[when]);
  $('every-weekday-wrap').hidden = $('every-frequency').value !== 'weekly';
  const kind = $('watch-kind').value;
  for (const field of document.querySelectorAll('[data-watch]')) field.hidden = field.dataset.watch !== kind;
}

/** The trigger the form describes for a When other than now. */
function triggerFor(when) {
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

// -- start -------------------------------------------------------------------------------------

function refreshSoon() {
  clearInterval(state.timer);
  state.timer = setInterval(() => {
    // Not while an answer is being typed: rendering would replace the field.
    if (document.visibilityState !== 'visible' || $('now-list').contains(document.activeElement)) return;
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
  document.addEventListener('visibilitychange', () => { if (document.visibilityState === 'visible') load(); });
  // A day in the address moves the river there; other anchors (#now) only scroll.
  window.addEventListener('hashchange', () => {
    const hash = location.hash.slice(1);
    if (!hash || /^\d{4}-\d{2}-\d{2}$/.test(hash)) go(hash || null);
  });

  whenChanged();
  refreshSoon();
  const day = location.hash.slice(1);
  go(/^\d{4}-\d{2}-\d{2}$/.test(day) ? day : null);
}

start();

'use strict';

// Agents, shared by every UI: prompts that Cedar runs on its own, each with a trigger. This is
// one page, drawn over the chat and addressed by the hash, so a view can be linked to and the
// browser's back button works:
//
//   #/agents               every agent, grouped by what starts it
//   #/agents/activity      the latest runs of all of them, with what started each
//   #/agents/new           the form (?from=<conversation id> starts from that chat: cedar writes
//                          one prompt for the whole conversation, which the user can edit)
//   #/agents/<id>          one agent: its trigger, prompt, actions and runs
//   #/agents/<id>/edit     the form again
//
// It also keeps the link to the page in the sidebar up to date (every `[data-agents-nav]`), and
// turns the badge on a run's conversation into a link to its agent.
//
// Each app hands over its own helpers through cedarAgents.init(), so this file knows nothing
// about the app's internals; calls cedarAgents.start() once the access token is known; and asks
// cedarAgents.route() first in its own router, which says whether the hash was one of the above:
//
//   api(method, path, body) -> Promise<json>   the app's /api client (sends the access token)
//   toast(message)                             a short notice
//   renderMarkdown(s) -> html string           the app's Markdown renderer, which escapes all text
//
// The model and reasoning pickers take cedar's registry from cedarModels (models.js); the ways
// to be notified come from cedarNotify (notify.js).

const cedarAgents = (() => {
  /** Monday to Sunday in the page's language (1 January 2024 was a Monday). */
  const DAYS = Array.from({ length: 7 }, (_, i) => new Date(2024, 0, 1 + i).toLocaleDateString(cedarI18n.locale, { weekday: 'long' }));
  const ACTIVE = new Set(['queued', 'running']);
  const PREVIEW_CHARS = 600;
  const REFRESH_MS = 10_000;
  const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
  /** The kinds of trigger, by the group the list shows them in. */
  const GROUPS = { schedule: ['time', 'once', 'interval', 'monthly'], condition: ['condition'], webhook: ['webhook'], mail: ['email', 'calendar'] };
  /** What the form's picker offers: mail and calendar are one group in the list, two choices here. */
  const PICKS = ['schedule', 'condition', 'webhook', 'email', 'calendar'];
  const FREQUENCIES = ['daily', 'weekdays', 'weekly', 'monthly', 'interval', 'once'];
  const OPS = ['changes', 'lt', 'lte', 'gt', 'gte', 'equals', 'contains'];
  const SIGNS = { lt: '<', lte: '≤', gt: '>', gte: '≥', equals: '=' };
  const ICONS = {
    schedule: '<circle cx="12" cy="12" r="9"></circle><path d="M12 7v5l3 2"></path>',
    condition: '<path d="M3 12h4l3-8 4 16 3-8h4"></path>',
    webhook: '<path d="M13 2L4 14h7l-1 8 9-12h-7z"></path>',
    email: '<rect x="3" y="5" width="18" height="14" rx="2"></rect><path d="M3 7l9 6 9-6"></path>',
    calendar: '<rect x="3" y="5" width="18" height="16" rx="2"></rect><path d="M3 10h18M8 3v4M16 3v4"></path>',
    mail: '<rect x="3" y="5" width="18" height="14" rx="2"></rect><path d="M3 7l9 6 9-6"></path>',
    back: '<path d="M15 6l-6 6 6 6"></path>',
    agents: '<rect x="4" y="8" width="16" height="11" rx="3"></rect><path d="M12 8V4M9 13h.01M15 13h.01M2 13v2M22 13v2"></path>',
  };

  let deps = null;
  let page = null;       // the page's elements, built on first use
  let started = false;
  let agents = [];       // GET /agents, as last loaded
  let limits = {};
  let unread = new Set(); // ids of agents with a result nobody has looked at
  let view = 0;          // bumped on every render, so a slow fetch can't draw into a newer view
  let refreshTimer = null;
  let lastChat = '';     // the hash to go back to
  let prefill = null;    // { conversation, model, reasoning } handed over by "Make an agent"
  let fresh = null;      // { id, secret } of a webhook secret that was just made, to show once

  // -- helpers -----------------------------------------------------------------------------------

  function el(tag, className, text) {
    const e = document.createElement(tag);
    if (className) e.className = className;
    if (text !== undefined) e.textContent = text;
    return e;
  }

  function button(label, onClick, className = 'copy-btn') {
    const b = el('button', className, label);
    b.type = 'button';
    b.addEventListener('click', onClick);
    return b;
  }

  function link(label, href, className) {
    const a = el('a', className, label);
    a.href = href;
    return a;
  }

  function icon(name) {
    const span = el('span', 'cedar-ag-icon');
    span.innerHTML = `<svg viewBox="0 0 24 24" aria-hidden="true">${ICONS[name] || ICONS.schedule}</svg>`;
    return span;
  }

  function localZone() {
    try { return Intl.DateTimeFormat().resolvedOptions().timeZone || 'UTC'; } catch { return 'UTC'; }
  }

  /** "Mon 12 Oct, 09:00" in the viewer's locale. */
  function when(iso) {
    if (!iso) return '';
    return new Date(iso).toLocaleString(cedarI18n.locale, { weekday: 'short', day: 'numeric', month: 'short', hour: '2-digit', minute: '2-digit' });
  }

  /** A trigger's own local time ("2026-12-03T09:00"), read as the wall clock it names. */
  function wallClock(at) {
    const m = /^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2})/.exec(at || '');
    if (!m) return at || '';
    const d = new Date(Number(m[1]), Number(m[2]) - 1, Number(m[3]), Number(m[4]), Number(m[5]));
    return d.toLocaleString(cedarI18n.locale, { weekday: 'short', day: 'numeric', month: 'short', year: 'numeric', hour: '2-digit', minute: '2-digit' });
  }

  function groupOf(kind) {
    return Object.keys(GROUPS).find((g) => GROUPS[g].includes(kind)) || 'schedule';
  }

  /** The picker's choice for a kind of trigger. */
  function pickOf(kind) {
    return GROUPS.schedule.includes(kind) ? 'schedule' : PICKS.includes(kind) ? kind : 'schedule';
  }

  /** Whether something outside starts the runs (not the clock): these get limits and careful tools. */
  function byEvent(kind) {
    return ['condition', 'webhook', 'email', 'calendar'].includes(kind);
  }

  /** "Every 2 hours", from minutes. */
  function every(minutes) {
    if (minutes % 1440 === 0) return tr('agents.at.everyDays', { count: minutes / 1440 });
    if (minutes % 60 === 0) return tr('agents.at.everyHours', { count: minutes / 60 });
    return tr('agents.at.everyMinutes', { count: minutes });
  }

  /** What a condition watches, in a few words. */
  function watched(source, path) {
    const s = source || {};
    if (s.type === 'url') {
      let host = s.url || '';
      try { host = new URL(s.url).host; } catch { /* shown as typed */ }
      return path ? tr('agents.what.urlPath', { path, host }) : tr('agents.what.url', { host });
    }
    if (s.type === 'tool') return path ? tr('agents.what.toolPath', { path, name: s.name }) : tr('agents.what.tool', { name: s.name });
    if (s.type === 'records') return path ? tr('agents.what.recordsPath', { path, set: s.set }) : tr('agents.what.records', { set: s.set });
    return '';
  }

  /** "Every weekday at 09:00 (Europe/Berlin)": a trigger in a line. */
  function describe(t) {
    if (!t) return '';
    const zone = t.timezone && t.timezone !== localZone() ? ` (${t.timezone})` : '';
    switch (t.kind) {
      case 'time':
        if (t.frequency === 'weekly' && t.weekday != null) return tr('agents.at.weeklyOn', { day: DAYS[t.weekday], time: t.time }) + zone;
        return tr(t.frequency === 'weekdays' ? 'agents.at.weekdays' : 'agents.at.daily', { time: t.time }) + zone;
      case 'monthly': return tr('agents.at.monthly', { day: t.day, time: t.time }) + zone;
      case 'once': return tr('agents.at.once', { when: wallClock(t.at) }) + zone;
      case 'interval': return every(t.every_minutes);
      case 'condition': {
        const what = watched(t.source, t.extract);
        const op = t.predicate?.op;
        const value = t.predicate?.value;
        let text;
        if (op === 'changes') text = tr('agents.desc.changes', { what });
        else if (op === 'contains') text = tr('agents.desc.contains', { what, value: String(value) });
        else text = tr('agents.desc.compare', { what, op: SIGNS[op] || op, value: typeof value === 'number' ? cedarI18n.number(value) : String(value) });
        return `${text} · ${tr('agents.checkEvery', { count: t.every_minutes })}`;
      }
      case 'webhook': return tr('agents.desc.webhook');
      case 'email': return t.query ? tr('agents.desc.email', { query: t.query }) : tr('agents.desc.emailAny');
      case 'calendar': {
        const soon = t.minutes_before == null ? null : t.minutes_before === 0 ? tr('agents.desc.calStart') : tr('agents.desc.calSoon', { count: t.minutes_before });
        let what = soon && t.new_events ? tr('agents.desc.calBoth', { soon }) : soon || tr('agents.desc.calNew');
        if (t.filter) what = tr('agents.desc.calFilter', { what, filter: t.filter });
        return what;
      }
      default: return tr('agents.desc.unknown', { kind: t.kind });
    }
  }

  /** Where an agent stands: a line and a class (ok, err, live, muted). */
  function status(a) {
    const kind = a.trigger?.kind;
    if (!a.enabled) return [kind === 'once' && a.last_run_at ? tr('agents.status.done') : tr('agents.status.paused'), 'muted'];
    if (a.last_error) return [tr('agents.status.failing'), 'err'];
    if (a.waiting) return [tr('agents.status.held'), 'live'];
    if (kind === 'webhook') return [tr('agents.status.listening'), 'ok'];
    if (byEvent(kind)) return [a.last_checked_at ? tr('agents.status.watchingChecked', { when: when(a.last_checked_at) }) : tr('agents.status.watching'), 'ok'];
    if (a.next_run_at) return [tr('agents.status.next', { when: when(a.next_run_at) }), ''];
    return ['', ''];
  }

  /** "Notifies by email". */
  function notifyText(a) {
    if (!a.notify) return '';
    if (a.notify === 'none') return tr('agents.onlyInApp');
    if (typeof cedarNotify === 'undefined') return tr('agents.notifiesBy', { method: a.notify });
    // German nouns keep their capital letter.
    const label = cedarNotify.methodLabel(a.notify);
    return tr('agents.notifiesBy', { method: cedarI18n.lang === 'de' ? label : label.toLowerCase() });
  }

  const stateName = (state) => (cedarI18n.has(`agents.state.${state}`) ? tr(`agents.state.${state}`) : state);

  /** What a run is up to: a label and a class (ok, err, live, muted). */
  function runState(run) {
    if (run.error) return [tr('agents.state.notStarted'), 'err'];
    const t = run.turn;
    if (!t) return [run.conversation_id ? tr('agents.state.noResponse') : tr('agents.state.deleted'), 'muted'];
    if (ACTIVE.has(t.status)) return [stateName(t.status), 'live'];
    if (t.status === 'completed') return [stateName('completed'), 'ok'];
    return [stateName(t.status), 'err'];
  }

  function chip(run) {
    const [label, cls] = runState(run);
    const c = el('span', `cedar-ag-chip ${cls}`, label);
    if (cls === 'live') c.prepend(el('span', 'spinner'));
    return c;
  }

  const isLive = (run) => runState(run)[1] === 'live';

  /** What started a run, as a sentence; nothing for the clock, which goes without saying. */
  function causeText(run) {
    if (run.manual) return tr('agents.cause.manual');
    const summary = run.cause?.summary;
    if (!summary || run.cause.kind === 'clock') return '';
    return summary.charAt(0).toLocaleUpperCase(cedarI18n.locale) + summary.slice(1);
  }

  /** One run: when, how it went, what started it, and the start of its result. */
  function runRow(run, { agent } = {}) {
    const row = el('div', 'cedar-ag-run');
    const head = el('div', 'cedar-ag-run-head');
    if (agent) head.append(link(agent.title, `#/agents/${agent.id}`, 'cedar-ag-run-agent'), el('span', 'cedar-ag-when dim', when(run.started_at)));
    else head.append(el('span', 'cedar-ag-when', when(run.started_at)));
    head.append(chip(run));
    if (run.conversation_id) head.append(button(tr('agents.open'), () => { location.hash = `#/c/${run.conversation_id}`; }));
    row.append(head);
    const cause = causeText(run);
    if (cause) {
      const line = el('div', 'cedar-ag-cause', cause);
      if (run.cause?.excerpt) {
        const more = el('details', 'cedar-ag-details');
        more.append(el('summary', '', tr('agents.cause.data')), el('pre', 'cedar-ag-pre', run.cause.excerpt));
        line.append(more);
      }
      row.append(line);
    }
    const body = el('div', 'cedar-ag-run-body');
    if (run.error) {
      body.append(el('div', 'cedar-ag-error', run.error));
    } else if (run.turn?.error) {
      body.append(el('div', 'cedar-ag-error', run.turn.error));
    } else if (run.turn?.final_text) {
      const text = run.turn.final_text;
      const md = el('div', 'md cedar-ag-result');
      const short = text.length > PREVIEW_CHARS;
      md.innerHTML = deps.renderMarkdown(short ? `${text.slice(0, PREVIEW_CHARS)}…` : text);
      body.append(md);
      if (short) {
        const more = button(tr('agents.showAll'), () => { md.innerHTML = deps.renderMarkdown(text); more.remove(); });
        body.append(more);
      }
    } else if (run.turn && ACTIVE.has(run.turn.status)) {
      body.append(el('div', 'cedar-ag-note', tr('agents.working')));
    }
    if (body.childElementCount) row.append(body);
    return row;
  }

  function stopRefresh() {
    clearTimeout(refreshTimer);
    refreshTimer = null;
  }

  /** Draw again in a while, as long as this view is still the one showing. */
  function refreshLater(draw) {
    const current = view;
    stopRefresh();
    refreshTimer = setTimeout(() => { if (current === view && !page.root.hidden) draw(); }, REFRESH_MS);
  }

  // -- the page ----------------------------------------------------------------------------------

  function build() {
    const root = el('section', 'cedar-ag-page');
    root.id = 'agents-page';
    root.hidden = true;
    root.setAttribute('aria-labelledby', 'agents-page-title');
    const wrap = el('div', 'cedar-ag-wrap');
    const top = el('div', 'cedar-ag-top');
    const back = button('', () => leave(), 'cedar-ag-back');
    back.innerHTML = `<svg viewBox="0 0 24 24" aria-hidden="true">${ICONS.back}</svg>`;
    back.append(el('span', '', tr('agents.back')));
    top.append(back);
    const body = el('div', 'cedar-ag-body');
    wrap.append(top, body);
    root.append(wrap);
    document.body.append(root);
    return { root, body, top };
  }

  /** Back to the chat that was open (or a new one). */
  function leave() {
    location.hash = lastChat && !lastChat.startsWith('#/agents') ? lastChat : '#/';
  }

  function show() {
    page ??= build();
    if (page.root.hidden) {
      page.root.hidden = false;
      page.root.scrollTop = 0;
    }
  }

  function hide() {
    stopRefresh();
    view++;
    if (page) page.root.hidden = true;
  }

  /** The heading of a view: a title, with what goes beside it. */
  function heading(title, ...beside) {
    const head = el('div', 'cedar-ag-head');
    const h1 = el('h1', '', title);
    h1.id = 'agents-page-title';
    head.append(h1, ...beside);
    return head;
  }

  function tabs(current) {
    const nav = el('nav', 'cedar-ag-tabs');
    for (const [id, href] of [['agents', '#/agents'], ['activity', '#/agents/activity']]) {
      const a = link(tr(`agents.tab.${id}`), href, 'cedar-ag-tab');
      if (id === current) a.setAttribute('aria-current', 'page');
      nav.append(a);
    }
    return nav;
  }

  /**
   * Handle the hash if it is one of the page's, and say whether it was. Each app's router asks
   * this first, and shows its own view only when the answer is no.
   */
  function route() {
    const m = location.hash.match(/^#\/agents(?:\/([^?]*))?(?:\?(.*))?$/);
    if (!m || !deps) {
      hide();
      lastChat = location.hash;
      return false;
    }
    const parts = (m[1] || '').split('/').filter(Boolean);
    const query = new URLSearchParams(m[2] || '');
    show();
    stopRefresh();
    page.root.scrollTop = 0;
    page.top.replaceChildren(page.top.firstElementChild);
    if (parts[0] === 'activity') viewActivity();
    else if (parts[0] === 'new') viewForm({ from: query.get('from') });
    else if (UUID.test(parts[0] || '')) parts[1] === 'edit' ? viewForm({ id: parts[0] }) : viewDetail(parts[0]);
    else viewList();
    return true;
  }

  // -- every agent -------------------------------------------------------------------------------

  /** Fetch the agents (and which have unseen results), and bring the sidebar links up to date. */
  async function load() {
    if (!deps) return;
    try {
      const [list, inbox] = await Promise.all([deps.api('GET', '/agents'), deps.api('GET', '/inbox').catch(() => null)]);
      agents = list.agents || [];
      limits = list.limits || {};
      unread = new Set((inbox?.items || []).map((i) => i.schedule_id).filter(Boolean));
    } catch { return; }
    renderNav();
  }

  function renderNav() {
    for (const nav of document.querySelectorAll('[data-agents-nav]')) {
      const count = nav.querySelector('.cedar-ag-count');
      if (count) count.textContent = agents.length ? String(agents.length) : '';
      const dot = nav.querySelector('.cedar-ag-dot');
      if (dot) dot.hidden = !unread.size;
    }
  }

  function card(a) {
    const kind = a.trigger?.kind;
    const c = link('', `#/agents/${a.id}`, `cedar-ag-card${a.enabled ? '' : ' paused'}`);
    const title = el('div', 'cedar-ag-card-title');
    title.append(el('span', '', a.title));
    if (unread.has(a.id)) {
      const dot = el('span', 'cedar-ag-dot');
      dot.title = tr('agents.unread');
      title.append(dot);
    }
    const meta = el('div', 'cedar-ag-meta');
    const [text, cls] = status(a);
    if (text) meta.append(el('span', `cedar-ag-status ${cls}`, text));
    const last = a.last_run;
    if (last) meta.append(chip(last), el('span', '', tr('agents.lastRun', { when: when(last.started_at) })));
    else meta.append(el('span', '', tr('agents.neverRan')));
    c.append(icon(groupOf(kind) === 'mail' ? kind : groupOf(kind)), title, el('div', 'cedar-ag-sub', describe(a.trigger)), meta);
    const snippet = a.last_error || last?.error || last?.turn?.error || last?.turn?.final_text;
    if (snippet) c.append(el('div', 'cedar-ag-snippet', String(snippet).replace(/[#*_`>]/g, '').slice(0, 300)));
    return c;
  }

  async function viewList() {
    const current = ++view;
    const add = link(tr('agents.new'), '#/agents/new', 'primary');
    const parts = [heading(tr('agents.title'), add), el('p', 'cedar-ag-intro', tr('agents.intro')), tabs('agents')];
    if (!agents.length) page.body.replaceChildren(...parts, el('div', 'cedar-ag-note', tr('agents.loading')));
    await load();
    if (current !== view) return;
    if (!agents.length) {
      const empty = el('div', 'cedar-ag-empty');
      empty.append(el('div', '', tr('agents.empty')), link(tr('agents.new'), '#/agents/new', 'primary'));
      page.body.replaceChildren(...parts, empty);
      return;
    }
    for (const group of Object.keys(GROUPS)) {
      const mine = agents.filter((a) => groupOf(a.trigger?.kind) === group);
      if (!mine.length) continue;
      const section = el('section', 'cedar-ag-group');
      const cards = el('div', 'cedar-ag-cards');
      cards.append(...mine.map(card));
      section.append(el('h2', 'cedar-ag-group-label', tr(`agents.group.${group}`)), cards);
      parts.push(section);
    }
    page.body.replaceChildren(...parts);
    if (agents.some((a) => a.last_run && isLive(a.last_run))) refreshLater(viewList);
  }

  // -- the latest runs of all of them -------------------------------------------------------------

  async function viewActivity() {
    const current = ++view;
    const add = link(tr('agents.new'), '#/agents/new', 'primary');
    const parts = [heading(tr('agents.title'), add), el('p', 'cedar-ag-intro', tr('agents.intro')), tabs('activity')];
    const list = el('div', 'cedar-ag-runs');
    const foot = el('div', 'cedar-ag-actions');
    page.body.replaceChildren(...parts, el('div', 'cedar-ag-note', tr('agents.loading')));
    let next = null;
    let live = false;
    const more = async () => {
      let data;
      try {
        data = await deps.api('GET', `/agents/activity${next ? `?before=${encodeURIComponent(next)}` : ''}`);
      } catch (e) {
        if (current === view) page.body.replaceChildren(...parts, el('div', 'cedar-ag-error', e.message || tr('agents.loadFailed')));
        return false;
      }
      if (current !== view) return false;
      for (const item of data.items || []) {
        live ||= isLive(item);
        list.append(runRow(item, { agent: item.agent }));
      }
      next = data.next_before;
      foot.replaceChildren(...(next ? [button(tr('agents.activity.more'), () => more())] : []));
      return true;
    };
    if (!(await more())) return;
    if (!list.childElementCount) {
      const empty = el('div', 'cedar-ag-empty');
      empty.append(el('div', '', tr('agents.activity.empty')));
      page.body.replaceChildren(...parts, empty);
    } else {
      page.body.replaceChildren(...parts, list, foot);
    }
    load();
    if (live) refreshLater(viewActivity);
  }

  // -- one agent ---------------------------------------------------------------------------------

  async function copy(text, btn) {
    try {
      await navigator.clipboard.writeText(text);
      const label = btn.textContent;
      btn.textContent = tr('agents.hook.copied');
      setTimeout(() => { btn.textContent = label; }, 1500);
    } catch {
      deps.toast(text);
    }
  }

  function kv(label, value) {
    const code = el('code', '', value);
    return [el('span', '', label), code, button(tr('agents.hook.copy'), (e) => copy(value, e.currentTarget))];
  }

  /** A webhook agent's address, its secret (in full only right after it was made), and how to call it. */
  function hookBox(a) {
    const box = el('div', 'cedar-ag-box');
    const url = `${location.origin}${a.hook.path}`;
    const secret = fresh?.id === a.id ? fresh.secret : null;
    const grid = el('div', 'cedar-ag-kv');
    grid.append(...kv(tr('agents.hook.url'), url));
    if (secret) grid.append(...kv(tr('agents.hook.secret'), secret));
    else grid.append(el('span', '', tr('agents.hook.secret')), el('span', 'cedar-ag-note', tr('agents.hook.secretEnds', { last4: a.hook.secret_last4 || '····' })), button(tr('agents.hook.rotate'), () => rotate(a)));
    box.append(el('h2', '', tr('agents.hook.title')), grid);
    if (secret) {
      box.classList.add('warn');
      box.append(el('div', '', tr('agents.hook.secretOnce')));
    }
    const example = `curl -X POST '${url}' \\\n  -H 'Authorization: Bearer ${secret || '<secret>'}' \\\n  -H 'Content-Type: application/json' \\\n  -d '{"door": "open"}'`;
    const how = el('details', 'cedar-ag-details');
    how.open = !!secret;
    how.append(el('summary', '', tr('agents.hook.example')), el('pre', 'cedar-ag-pre', example));
    box.append(el('div', 'cedar-ag-note', `${tr('agents.hook.how')} ${tr('agents.hook.reach')}`), how);
    return box;
  }

  async function viewDetail(id) {
    const current = ++view;
    const known = agents.find((a) => a.id === id);
    const all = link(tr('agents.allAgents'), '#/agents', 'cedar-ag-tab');
    if (!page.body.querySelector(`[data-agent="${id}"]`)) {
      page.body.replaceChildren(heading(known?.title || tr('agents.title')), el('div', 'cedar-ag-note', tr('agents.loading')));
    }
    let data;
    try {
      data = await deps.api('GET', `/agents/${id}`);
    } catch (e) {
      if (current === view) page.body.replaceChildren(heading(tr('agents.title')), el('div', 'cedar-ag-error', e.message || tr('agents.loadFailed')), all);
      return;
    }
    if (current !== view) return;
    const { agent: a, runs } = data;
    const again = () => viewDetail(id);
    const facts = el('div', 'cedar-ag-facts');
    facts.dataset.agent = id;
    facts.append(el('div', 'cedar-ag-sub', describe(a.trigger)));
    const [text, cls] = status(a);
    if (text) facts.append(el('div', `cedar-ag-status ${cls}`, text));
    if (a.last_error) facts.append(el('div', 'cedar-ag-error', tr('agents.checkFailed', { error: a.last_error })));
    const small = [notifyText(a)];
    if (byEvent(a.trigger?.kind)) {
      const gap = Math.round((a.min_interval_secs ?? limits.min_interval_secs ?? 300) / 60);
      small.push(tr('agents.limitsText', { interval: tr('agents.minutesShort', { count: gap }), count: a.max_runs_per_day ?? limits.max_runs_per_day ?? 24 }));
    }
    small.push(`${tr('agents.toolsLabel')}: ${a.tools ? a.tools.join(', ') : tr('agents.toolsDefault')}`);
    facts.append(el('div', 'cedar-ag-note', small.filter(Boolean).join(' · ')));

    const actions = el('div', 'cedar-ag-actions');
    actions.append(
      button(tr('agents.runNow'), (e) => runNow(a, e.currentTarget, again)),
      button(a.enabled ? tr('agents.pause') : tr('agents.resume'), () => setEnabled(a, !a.enabled, again)),
      button(tr('agents.edit'), () => { location.hash = `#/agents/${a.id}/edit`; }),
      button(tr('agents.delete'), () => remove(a)),
    );
    if (a.source_conversation_id) actions.append(button(tr('agents.openSource'), () => { location.hash = `#/c/${a.source_conversation_id}`; }));
    const promptBox = el('details', 'cedar-ag-details');
    promptBox.append(el('summary', '', tr('agents.prompt')), el('pre', 'cedar-ag-pre', a.prompt));

    const parts = [heading(a.title), facts, actions];
    if (a.hook) parts.push(hookBox(a));
    parts.push(promptBox, el('div', 'cedar-ag-runs-label', tr('agents.runs', { count: runs.length })));
    if (!runs.length) parts.push(el('div', 'cedar-ag-note', tr('agents.noRuns')));
    const list = el('div', 'cedar-ag-runs');
    list.append(...runs.map((run) => runRow(run)));
    parts.push(list);
    const keepOpen = page.body.querySelector('.cedar-ag-details[open] > summary')?.textContent;
    page.body.replaceChildren(...parts);
    // A redraw shouldn't close what the reader had open.
    if (keepOpen === tr('agents.prompt')) promptBox.open = true;
    page.top.replaceChildren(page.top.firstElementChild, all);
    if (runs.some(isLive)) refreshLater(again);
  }

  async function runNow(a, btn, then) {
    if (btn) btn.disabled = true;
    try {
      await deps.api('POST', `/agents/${a.id}/run`);
      deps.toast(tr('agents.started', { title: a.title }));
      then();
    } catch (e) {
      deps.toast(e.message);
      if (btn) btn.disabled = false;
    }
  }

  async function setEnabled(a, enabled, then) {
    try {
      await deps.api('PATCH', `/agents/${a.id}`, { enabled });
      load();
      then();
    } catch (e) {
      deps.toast(e.message);
    }
  }

  async function remove(a) {
    if (!(await cedarConfirm(tr('agents.confirmDelete', { title: a.title }), { ok: tr('agents.delete') }))) return;
    try {
      await deps.api('DELETE', `/agents/${a.id}`);
      agents = agents.filter((x) => x.id !== a.id);
      renderNav();
      location.hash = '#/agents';
    } catch (e) {
      deps.toast(e.message);
    }
  }

  async function rotate(a) {
    if (!(await cedarConfirm(tr('agents.hook.confirmRotate'), { ok: tr('agents.hook.rotate') }))) return;
    try {
      const updated = await deps.api('POST', `/agents/${a.id}/hook/rotate`);
      fresh = { id: a.id, secret: updated.hook.secret };
      viewDetail(a.id);
    } catch (e) {
      deps.toast(e.message);
    }
  }

  // -- the form: a new agent, or an edited one -----------------------------------------------------

  function field(labelText, control) {
    const wrap = el('label', 'cedar-ag-field');
    wrap.append(el('span', '', labelText), control);
    return wrap;
  }

  function input(type, attrs = {}) {
    const i = el('input');
    i.type = type;
    Object.assign(i, attrs);
    return i;
  }

  function select(options, value) {
    const s = el('select');
    for (const [v, label] of options) s.append(new Option(label, v));
    if (value !== undefined) s.value = value;
    return s;
  }

  function help(text) {
    const h = el('div', 'cedar-ag-help');
    h.append(el('span', '', text));
    return h;
  }

  /** The next full hour, as HH:MM. */
  function defaultTime() {
    const d = new Date();
    d.setHours(d.getHours() + 1, 0, 0, 0);
    return `${String(d.getHours()).padStart(2, '0')}:00`;
  }

  function today() {
    const d = new Date();
    return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`;
  }

  /** "Check every [5] minutes", shared by the triggers that watch. */
  function everyField(value) {
    const n = input('number', { min: 1, max: 10080, step: 1, required: true, value: String(value ?? limits.check_minutes ?? 5) });
    return { wrap: field(`${tr('agents.form.checkEvery')} (${tr('agents.unit.minutes')})`, n), n };
  }

  async function viewForm({ id, from }) {
    const current = ++view;
    page.top.replaceChildren(page.top.firstElementChild, link(tr('agents.allAgents'), '#/agents', 'cedar-ag-tab'));
    page.body.replaceChildren(heading(id ? tr('agents.form.edit') : tr('agents.form.new')), el('div', 'cedar-ag-note', tr('agents.loading')));
    let agent = null;
    let conversation = null;
    try {
      const needs = [deps.api('GET', '/tools').catch(() => ({})), deps.api('GET', '/records').catch(() => ({}))];
      if (id) needs.push(deps.api('GET', `/agents/${id}`));
      else if (from && UUID.test(from)) needs.push(prefill?.conversation?.id === from ? { conversation: prefill.conversation } : deps.api('GET', `/conversations/${from}`));
      if (!agents.length && !Object.keys(limits).length) needs.push(load());
      const [catalog, records, third] = await Promise.all(needs);
      if (current !== view) return;
      if (id) agent = third.agent;
      else if (third?.conversation) conversation = third.conversation;
      drawForm({ current, agent, conversation, catalog, records });
    } catch (e) {
      if (current === view) page.body.replaceChildren(heading(tr('agents.title')), el('div', 'cedar-ag-error', e.message || tr('agents.loadFailed')));
    }
  }

  function drawForm({ current, agent, conversation, catalog, records }) {
    const t = agent?.trigger || {};
    const zone = t.timezone || localZone();
    const pollable = (catalog.groups || []).flatMap((g) => (g.tools || []).filter((tool) => tool.pollable).map((tool) => ({ ...tool, personal: g.id === 'custom' })));
    const sets = (records.sets || []).map((s) => s.name);

    // Name and prompt; with a conversation to start from, cedar writes the prompt first.
    const name = input('text', { maxLength: 200, required: true, placeholder: tr('agents.form.namePlaceholder'), value: agent?.title || conversation?.title || '' });
    const prompt = el('textarea');
    prompt.rows = 8;
    prompt.required = true;
    prompt.placeholder = tr('agents.form.promptPlaceholder');
    prompt.value = agent?.prompt || '';
    const promptHelp = help(tr('agents.form.promptHelp'));
    if (conversation) promptHelp.append(button(tr('agents.form.regenerate'), () => compact()));
    const review = el('div', 'cedar-ag-review');
    const reviewText = el('span', '', tr('agents.form.reviewing'));
    const retry = button(tr('agents.form.tryAgain'), () => compact());
    const skip = button(tr('agents.form.writeMyself'), () => { review.hidden = true; form.hidden = false; prompt.focus(); });
    review.append(el('span', 'spinner'), reviewText, retry, skip);
    review.hidden = true;

    // What starts it: one choice of five, each with its own fields below.
    const picks = el('div', 'cedar-ag-picks');
    picks.setAttribute('role', 'radiogroup');
    const radios = {};
    for (const pick of PICKS) {
      const label = el('label', 'cedar-ag-pick');
      const radio = input('radio', { name: 'cedar-ag-pick', value: pick });
      radios[pick] = radio;
      label.append(radio, icon(pick), el('b', '', tr(`agents.pick.${pick}`)), el('small', '', tr(`agents.pickHelp.${pick}`)));
      picks.append(label);
    }
    radios[pickOf(t.kind)].checked = true;
    const picked = () => PICKS.find((p) => radios[p].checked);

    // -- on a schedule
    const frequency = select(FREQUENCIES.map((f) => [f, tr(`agents.freq.${f}`)]), t.kind === 'time' ? t.frequency : GROUPS.schedule.includes(t.kind) ? t.kind : 'daily');
    const weekday = select(DAYS.map((d, i) => [String(i), d]), String(t.weekday ?? (new Date().getDay() + 6) % 7));
    const time = input('time', { required: true, value: t.time || (t.kind === 'once' ? t.at.slice(11, 16) : defaultTime()) });
    const day = input('number', { min: 1, max: 31, step: 1, value: String(t.day ?? new Date().getDate()) });
    const date = input('date', { value: t.kind === 'once' ? t.at.slice(0, 10) : today(), min: today() });
    const unitOf = (m) => (m && m % 1440 === 0 ? 1440 : m && m % 60 === 0 ? 60 : 1);
    const unit = select([['1', tr('agents.unit.minutes')], ['60', tr('agents.unit.hours')], ['1440', tr('agents.unit.days')]], String(t.kind === 'interval' ? unitOf(t.every_minutes) : 60));
    const amount = input('number', { min: 1, step: 1, value: String(t.kind === 'interval' ? t.every_minutes / unitOf(t.every_minutes) : 4) });
    const fields = {
      weekday: field(tr('agents.form.weekday'), weekday),
      day: field(tr('agents.form.dayOfMonth'), day),
      date: field(tr('agents.form.date'), date),
      time: field(tr('agents.form.time'), time),
      amount: field(tr('agents.form.every'), amount),
      unit: field(' ', unit),
    };
    const scheduleRow = el('div', 'cedar-ag-row');
    scheduleRow.append(field(tr('agents.form.howOften'), frequency), ...Object.values(fields));
    const scheduleNote = help('');
    const scheduleSet = el('fieldset', 'cedar-ag-set');
    scheduleSet.append(scheduleRow, scheduleNote);
    const syncSchedule = () => {
      const f = frequency.value;
      fields.weekday.hidden = f !== 'weekly';
      fields.day.hidden = f !== 'monthly';
      fields.date.hidden = f !== 'once';
      fields.time.hidden = f === 'interval';
      fields.amount.hidden = fields.unit.hidden = f !== 'interval';
      scheduleNote.firstChild.textContent = f === 'interval' ? tr('agents.form.intervalHelp', { minutes: limits.min_interval_minutes ?? 15 })
        : f === 'monthly' ? `${tr('agents.form.dayHelp')} ${tr('agents.form.zone', { zone })}` : tr('agents.form.zone', { zone });
    };
    frequency.addEventListener('change', syncSchedule);

    // -- when something changes
    const src = t.kind === 'condition' ? t.source : { type: 'url' };
    const sourceType = select([['url', tr('agents.source.url')], ['tool', tr('agents.source.tool')], ['records', tr('agents.source.records')]], src.type);
    const url = input('url', { placeholder: 'https://', value: src.url || '' });
    const urlAs = select([['', tr('agents.as.auto')], ['json', tr('agents.as.json')], ['text', tr('agents.as.text')]], src.as || '');
    const tool = el('select');
    for (const personal of [false, true]) {
      const some = pollable.filter((p) => p.personal === personal);
      if (!some.length) continue;
      const into = personal ? tool.appendChild(Object.assign(el('optgroup'), { label: tr('agents.form.personalGroup') })) : tool;
      for (const p of some) into.append(new Option(p.name, p.name));
    }
    if (src.type === 'tool' && ![...tool.options].some((o) => o.value === src.name)) tool.append(new Option(src.name, src.name));
    if (src.type === 'tool') tool.value = src.name;
    const toolInput = el('textarea', 'code');
    toolInput.rows = 3;
    toolInput.spellcheck = false;
    toolInput.value = src.type === 'tool' && Object.keys(src.input || {}).length ? JSON.stringify(src.input, null, 2) : '{}';
    const toolNote = el('div', 'cedar-ag-box warn');
    const set = el('select');
    for (const s of sets) set.append(new Option(s, s));
    if (src.type === 'records' && !sets.includes(src.set)) set.append(new Option(src.set, src.set));
    if (src.type === 'records') set.value = src.set;
    const sourceFields = {
      url: field(tr('agents.form.url'), url),
      urlAs: field(tr('agents.form.urlAs'), urlAs),
      tool: field(tr('agents.form.tool'), tool),
      set: field(tr('agents.form.set'), set),
    };
    const sourceRow = el('div', 'cedar-ag-row');
    sourceRow.append(field(tr('agents.form.source'), sourceType), ...Object.values(sourceFields));
    const toolInputField = field(tr('agents.form.toolInput'), toolInput);
    const sourceEmpty = el('div', 'cedar-ag-note');
    const path = input('text', { placeholder: tr('agents.form.pathPlaceholder'), value: t.extract || '', spellcheck: false, autocapitalize: 'off' });
    const pathHelp = help('');
    const op = select(OPS.map((o) => [o, tr(`agents.op.${o}`)]), t.predicate?.op || 'changes');
    const value = input('text', { value: t.predicate?.value != null ? String(t.predicate.value) : '' });
    const valueField = field(tr('agents.form.value'), value);
    const condEvery = everyField(t.kind === 'condition' ? t.every_minutes : undefined);
    const testRow = el('div', 'cedar-ag-row');
    testRow.append(field(tr('agents.form.when'), op), valueField, condEvery.wrap);
    const conditionSet = el('fieldset', 'cedar-ag-set');
    conditionSet.append(sourceRow, sourceEmpty, toolInputField, toolNote, field(tr('agents.form.path'), path), pathHelp, testRow);
    const isPersonal = () => sourceType.value === 'tool' && pollable.some((p) => p.personal && p.name === tool.value);
    const syncCondition = () => {
      const type = sourceType.value;
      sourceFields.url.hidden = sourceFields.urlAs.hidden = type !== 'url';
      sourceFields.tool.hidden = type !== 'tool' || !tool.options.length;
      toolInputField.hidden = sourceFields.tool.hidden;
      sourceFields.set.hidden = type !== 'records' || !set.options.length;
      sourceEmpty.hidden = !((type === 'tool' && !tool.options.length) || (type === 'records' && !set.options.length));
      sourceEmpty.textContent = type === 'tool' ? tr('agents.form.noTools') : tr('agents.form.noSets');
      const least = limits.personal_check_minutes ?? 5;
      toolNote.hidden = !isPersonal();
      toolNote.textContent = tr('agents.form.personalNote', { minutes: least });
      condEvery.n.min = isPersonal() ? least : 1;
      if (isPersonal() && Number(condEvery.n.value) < least) condEvery.n.value = String(least);
      pathHelp.firstChild.textContent = type === 'records' ? tr('agents.form.pathHelpRecords') : tr('agents.form.pathHelp');
      valueField.hidden = op.value === 'changes';
      value.required = op.value !== 'changes';
      value.inputMode = ['lt', 'lte', 'gt', 'gte'].includes(op.value) ? 'decimal' : 'text';
    };
    for (const control of [sourceType, tool, op]) control.addEventListener('change', syncCondition);

    // -- a webhook
    const webhookSet = el('fieldset', 'cedar-ag-set');
    webhookSet.append(el('div', 'cedar-ag-note', `${tr('agents.form.webhookNote')} ${tr('agents.hook.reach')}`));

    // -- mail
    const query = input('text', { placeholder: tr('agents.form.queryPlaceholder'), value: t.kind === 'email' ? t.query : '', spellcheck: false });
    const mailEvery = everyField(t.kind === 'email' ? t.every_minutes : undefined);
    const mailRow = el('div', 'cedar-ag-row');
    mailRow.append(mailEvery.wrap);
    const mailSet = el('fieldset', 'cedar-ag-set');
    mailSet.append(field(tr('agents.form.query'), query), help(`${tr('agents.form.queryHelp')} ${tr('agents.form.googleNote')}`), mailRow);

    // -- the calendar
    const cal = t.kind === 'calendar' ? t : { minutes_before: 15, new_events: false };
    const calFilter = input('text', { placeholder: tr('agents.form.calFilterPlaceholder'), value: cal.filter || '' });
    const soonOn = input('checkbox', { checked: cal.minutes_before != null });
    const soonMinutes = input('number', { min: 0, max: 10080, step: 1, value: String(cal.minutes_before ?? 15) });
    const soon = el('label', 'cedar-ag-check');
    soon.append(soonOn, el('span', '', tr('agents.form.calSoon')), soonMinutes, el('span', '', tr('agents.form.minutesBefore')));
    const newOn = input('checkbox', { checked: !!cal.new_events });
    const fresher = el('label', 'cedar-ag-check');
    fresher.append(newOn, el('span', '', tr('agents.form.calNew')));
    const calEvery = everyField(t.kind === 'calendar' ? t.every_minutes : undefined);
    const calRow = el('div', 'cedar-ag-row');
    calRow.append(calEvery.wrap);
    const calendarSet = el('fieldset', 'cedar-ag-set');
    calendarSet.append(soon, fresher, field(tr('agents.form.calFilter'), calFilter), help(`${tr('agents.form.calFilterHelp')} ${tr('agents.form.googleNote')}`), calRow);
    soonOn.addEventListener('change', () => { soonMinutes.disabled = !soonOn.checked; });
    soonMinutes.disabled = !soonOn.checked;

    // Trying a watching trigger out before saving it.
    const tryBtn = button(tr('agents.form.try'), () => tryOut());
    const tryResult = el('div', 'cedar-ag-try-result');
    const tryBox = el('div', 'cedar-ag-try');
    const tryLine = el('div', 'cedar-ag-inline');
    tryLine.append(tryBtn);
    tryBox.append(tryLine, tryResult);

    // What an agent started from outside may do, and how often.
    const safety = el('div', 'cedar-ag-box warn');
    safety.append(el('h2', '', tr('agents.form.safetyTitle')), el('div', '', tr('agents.form.safetyText')));
    const toolsChoice = select(agent ? [['keep', tr('agents.tools.keep')], ['defaults', tr('agents.tools.defaults')]] : [['careful', tr('agents.tools.careful')], ['defaults', tr('agents.tools.defaults')]]);
    const toolsField = field(tr('agents.form.toolsChoice'), toolsChoice);
    const gap = input('number', { min: 1, max: 10080, step: 1, placeholder: String(Math.round((limits.min_interval_secs ?? 300) / 60)), value: agent?.min_interval_secs ? String(Math.round(agent.min_interval_secs / 60)) : '' });
    const perDay = input('number', { min: 1, max: 500, step: 1, placeholder: String(limits.max_runs_per_day ?? 24), value: agent?.max_runs_per_day ? String(agent.max_runs_per_day) : '' });
    const limitsRow = el('div', 'cedar-ag-row');
    limitsRow.append(toolsField, field(tr('agents.form.minInterval'), gap), field(tr('agents.form.maxPerDay'), perDay));
    const limitsHelp = help(tr('agents.form.limitsHelp'));

    // Model, reasoning, and how a finished run is announced.
    const model = el('select');
    const modelField = field(tr('agents.form.model'), model);
    const reasoning = el('select');
    const reasoningField = field(tr('agents.form.reasoning'), reasoning);
    cedarModels.bindPair(model, reasoning);
    model.addEventListener('change', () => { reasoningField.hidden = reasoning.hidden; });
    const chosenModel = agent?.model || prefill?.model || conversation?.model || '';
    modelField.hidden = !cedarModels.fillSelect(model, { selected: chosenModel });
    model.hidden = false;
    reasoningField.hidden = !cedarModels.fillLevels(reasoning, model.value, agent?.reasoning || prefill?.reasoning || conversation?.reasoning || '');
    const notify = el('select');
    const notifyField = field(tr('agents.form.notifyMeBy'), notify);
    const runRow2 = el('div', 'cedar-ag-row');
    runRow2.append(modelField, reasoningField, notifyField);
    fillNotify(notify, agent?.notify ?? null);

    const error = el('div', 'cedar-ag-error');
    error.hidden = true;
    const submit = el('button', 'primary', agent ? tr('agents.form.save') : tr('agents.form.create'));
    submit.type = 'submit';
    const actions = el('div', 'cedar-ag-form-actions');
    actions.append(button(tr('agents.form.cancel'), () => { location.hash = agent ? `#/agents/${agent.id}` : '#/agents'; }), submit);

    const form = el('form', 'cedar-ag-form');
    form.append(
      field(tr('agents.form.name'), name), field(tr('agents.form.prompt'), prompt), promptHelp,
      el('div', 'cedar-ag-legend', tr('agents.form.trigger')), picks,
      scheduleSet, conditionSet, webhookSet, mailSet, calendarSet, tryBox,
      safety, limitsRow, limitsHelp, runRow2, help(tr('agents.form.notifyHelp')), error, actions,
    );
    const sets2 = { schedule: scheduleSet, condition: conditionSet, webhook: webhookSet, email: mailSet, calendar: calendarSet };
    const syncPick = () => {
      const pick = picked();
      for (const [id, fieldset] of Object.entries(sets2)) {
        fieldset.hidden = id !== pick;
        // A hidden field must not hold the form back.
        fieldset.disabled = id !== pick;
      }
      const outside = pick !== 'schedule';
      tryBox.hidden = !['condition', 'email', 'calendar'].includes(pick);
      safety.hidden = !outside;
      // On a clock, a new agent takes the chat's tools (or the defaults): nothing to choose.
      toolsField.hidden = !agent && !outside;
      gap.closest('.cedar-ag-field').hidden = perDay.closest('.cedar-ag-field').hidden = !outside;
      limitsRow.hidden = toolsField.hidden && !outside;
      limitsHelp.hidden = !outside;
      tryResult.replaceChildren();
    };
    for (const radio of Object.values(radios)) radio.addEventListener('change', syncPick);
    syncSchedule();
    syncCondition();
    syncPick();

    /** The trigger the form describes. Throws with a message when it can't be one. */
    function trigger() {
      const everyOf = (f) => Number(f.n.value);
      switch (picked()) {
        case 'schedule': {
          const f = frequency.value;
          if (f === 'interval') return { kind: 'interval', every_minutes: Number(amount.value) * Number(unit.value) };
          if (f === 'once') return { kind: 'once', at: `${date.value}T${time.value}`, timezone: zone };
          if (f === 'monthly') return { kind: 'monthly', day: Number(day.value), time: time.value, timezone: zone };
          const out = { kind: 'time', frequency: f, time: time.value, timezone: zone };
          if (f === 'weekly') out.weekday = Number(weekday.value);
          return out;
        }
        case 'condition': {
          let source;
          if (sourceType.value === 'url') {
            source = { type: 'url', url: url.value.trim() };
            if (urlAs.value) source.as = urlAs.value;
          } else if (sourceType.value === 'tool') {
            let parsed;
            try { parsed = JSON.parse(toolInput.value.trim() || '{}'); } catch { throw new Error(tr('agents.form.invalidJson')); }
            source = { type: 'tool', name: tool.value, input: parsed };
          } else {
            source = { type: 'records', set: set.value };
          }
          const predicate = { op: op.value };
          if (op.value !== 'changes') predicate.value = ['lt', 'lte', 'gt', 'gte'].includes(op.value) ? Number(value.value.replace(',', '.')) : value.value.trim();
          const out = { kind: 'condition', source, predicate, every_minutes: everyOf(condEvery) };
          if (path.value.trim()) out.extract = path.value.trim();
          return out;
        }
        case 'webhook': return { kind: 'webhook' };
        case 'email': return { kind: 'email', query: query.value.trim(), every_minutes: everyOf(mailEvery) };
        default: {
          if (!soonOn.checked && !newOn.checked) throw new Error(tr('agents.form.calNeed'));
          const out = { kind: 'calendar', new_events: newOn.checked, every_minutes: everyOf(calEvery) };
          if (soonOn.checked) out.minutes_before = Number(soonMinutes.value);
          if (calFilter.value.trim()) out.filter = calFilter.value.trim();
          return out;
        }
      }
    }

    async function tryOut() {
      let body;
      try { body = { trigger: trigger() }; } catch (e) { tryResult.replaceChildren(el('div', 'cedar-ag-error', e.message)); return; }
      tryBtn.disabled = true;
      tryResult.replaceChildren(el('div', 'cedar-ag-note', tr('agents.try.checking')));
      try {
        const r = await deps.api('POST', '/agents/test', body);
        if (current !== view) return;
        const out = [];
        if (!r.ok) {
          out.push(el('div', 'cedar-ag-error', r.error));
        } else {
          const kind = body.trigger.kind;
          const firstOnly = kind !== 'condition' ? !r.would_fire : body.trigger.predicate.op === 'changes';
          if (kind === 'email') out.push(el('b', '', tr('agents.try.threads', { count: Number(r.value) })));
          else if (kind === 'calendar') out.push(el('b', '', tr('agents.try.events', { count: Number(r.value) })));
          else out.push(el('b', '', tr('agents.try.value', { value: r.value })));
          if (r.would_fire) out.push(el('div', '', r.summary ? `${tr('agents.try.wouldFire')} (${r.summary})` : tr('agents.try.wouldFire')));
          else out.push(el('div', 'cedar-ag-note', firstOnly ? tr('agents.try.firstLook') : tr('agents.try.wouldNot')));
        }
        if (r.sample?.text) {
          const sample = el('details', 'cedar-ag-details');
          sample.open = !r.ok || (body.trigger.kind === 'condition' && !body.trigger.extract);
          sample.append(el('summary', '', tr('agents.try.sample')), el('pre', 'cedar-ag-pre', r.sample.text));
          if (r.sample.cut) sample.append(el('div', 'cedar-ag-note', tr('agents.try.cut')));
          out.push(sample);
        }
        tryResult.replaceChildren(...out);
      } catch (e) {
        if (current === view) tryResult.replaceChildren(el('div', 'cedar-ag-error', e.message));
      } finally {
        tryBtn.disabled = false;
      }
    }

    async function compact() {
      review.hidden = false;
      form.hidden = true;
      retry.hidden = true;
      skip.hidden = true;
      review.firstElementChild.hidden = false;
      reviewText.textContent = tr('agents.form.reviewing');
      try {
        const { prompt: written } = await deps.api('POST', `/conversations/${conversation.id}/compact`);
        if (current !== view) return;
        prompt.value = written;
        review.hidden = true;
        form.hidden = false;
        prompt.focus();
      } catch (e) {
        if (current !== view) return;
        review.firstElementChild.hidden = true;
        reviewText.textContent = e.message || tr('agents.form.reviewFailed');
        retry.hidden = false;
        skip.hidden = false;
      }
    }

    form.addEventListener('submit', async (e) => {
      e.preventDefault();
      error.hidden = true;
      const body = { title: name.value.trim(), prompt: prompt.value.trim(), timezone: localZone(), notify: notify.value || null };
      try { body.trigger = trigger(); } catch (err) {
        error.textContent = err.message;
        error.hidden = false;
        return;
      }
      if (!modelField.hidden) body.model = model.value;
      body.reasoning = reasoningField.hidden ? '' : reasoning.value;
      if (byEvent(body.trigger.kind)) {
        body.min_interval_secs = gap.value ? Number(gap.value) * 60 : null;
        body.max_runs_per_day = perDay.value ? Number(perDay.value) : null;
      }
      if (!toolsField.hidden && toolsChoice.value === 'defaults') body.tools = null;
      if (!agent && conversation) body.conversation_id = conversation.id;
      submit.disabled = true;
      try {
        const saved = agent ? await deps.api('PATCH', `/agents/${agent.id}`, body) : await deps.api('POST', '/agents', body);
        if (saved.hook?.secret) fresh = { id: saved.id, secret: saved.hook.secret };
        deps.toast(agent ? tr('agents.form.saved') : tr('agents.form.created', { title: saved.title }));
        prefill = null;
        load();
        location.hash = `#/agents/${saved.id}`;
      } catch (err) {
        error.textContent = err.message || tr('agents.form.saveFailed');
        error.hidden = false;
        submit.disabled = false;
      }
    });

    page.body.replaceChildren(heading(agent ? tr('agents.form.edit') : tr('agents.form.new')), review, form);
    if (conversation && !agent) compact();
    else name.focus();
  }

  /** Fill a notify picker with the methods that are set up; `selected` is an agent's choice (null: the default). */
  async function fillNotify(notify, selected) {
    const current = selected ?? '';
    notify.append(new Option(tr('agents.form.default'), ''), new Option(tr('agents.onlyInApp'), 'none'));
    notify.value = current === 'none' ? 'none' : '';
    if (typeof cedarNotify === 'undefined') return;
    try { await cedarNotify.loadStatus(true); } catch { return; }
    notify.textContent = '';
    for (const { value, label } of cedarNotify.methodOptions(true)) notify.append(new Option(label, value));
    if (current && ![...notify.options].some((o) => o.value === current)) {
      notify.append(new Option(tr('agents.form.noLongerSetUp', { method: cedarNotify.methodLabel(current) }), current));
    }
    notify.value = current;
  }

  // -- what the apps call --------------------------------------------------------------------------

  /** Open the form for a new agent made from the open conversation, with its model as chosen now. */
  function newFrom(conversation, { model, reasoning } = {}) {
    if (!conversation) return;
    prefill = { conversation, model, reasoning };
    location.hash = `#/agents/new?from=${conversation.id}`;
  }

  /**
   * Make `badge` (an `<a>`) the link from a run's conversation to its agent: hidden unless the
   * conversation is an agent's run. `agent` is `{id, title}` from GET /conversations/{id}.
   */
  function badge(node, conversation, agent) {
    if (!node) return;
    const id = agent?.id || conversation?.schedule_id;
    node.hidden = !id;
    if (!id) return;
    node.href = `#/agents/${id}`;
    const label = node.querySelector('.lbl') || node;
    label.textContent = agent?.title || tr('agents.badge');
    node.title = agent?.title ? tr('agents.badgeOf', { title: agent.title }) : tr('agents.badge');
  }

  function bind() {
    document.addEventListener('visibilitychange', () => { if (document.visibilityState === 'visible' && started) load(); });
    // Leaving with Escape, as a dialog would be left, unless one is open on top or a field has the focus.
    document.addEventListener('keydown', (e) => {
      if (e.key !== 'Escape' || !page || page.root.hidden || document.querySelector('dialog[open]')) return;
      if (/^(INPUT|TEXTAREA|SELECT)$/.test(document.activeElement?.tagName || '')) return;
      leave();
    });
  }

  function init(helpers) {
    deps = helpers;
    if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', bind);
    else bind();
  }

  /** Load the agents for the sidebar link; once, after the access token is known. */
  function start() {
    if (started || !deps) return;
    started = true;
    load();
  }

  /** A response finished somewhere: the count of unseen results may have changed. */
  function refresh() {
    if (started) load();
  }

  return { init, start, route, refresh, newFrom, badge, describe };
})();

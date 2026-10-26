'use strict';

// cedar-chat front end. Plain JavaScript, no build step. Talks only to /api on this origin.
//
// cedar's newer events show inline in the reply (trust-view.js draws them; an older cedar sends
// none): the plan it proposes, which it waits on until "start" (POST /api/turns/{id}/plan);
// approval cards for risky calls (POST /api/turns/{id}/approvals/{approval_id}); its reasoning,
// folded; [n] citations with the sources listed under the reply; and the cost and sandbox.

const $ = (id) => document.getElementById(id);
const els = {
  main: $('main'), sidebar: $('sidebar'), backdrop: $('backdrop'), convList: $('conv-list'),
  scroll: $('scroll'), thread: $('thread'), greeting: $('greeting'),
  title: $('title'), titleInput: $('title-input'), deleteChat: $('delete-chat'),
  composer: $('composer'), input: $('input'), send: $('send'), model: $('model'), reasoning: $('reasoning'), hint: $('hint'),
  toast: $('toast'), tokenDialog: $('token-dialog'), tokenForm: $('token-form'),
  tokenInput: $('token-input'), tokenMessage: $('token-message'),
  filesBtn: $('files-btn'), filesDialog: $('files-dialog'), filesList: $('files-list'),
  scheduleBtn: $('schedule-btn'), scheduleBadge: $('schedule-badge'), inboxBtn: $('inbox-btn'),
};

const TERMINAL = new Set(['run.completed', 'run.failed', 'run.cancelled']);
const ACTIVE = new Set(['queued', 'running']);
const TOKEN_KEY = 'cedar-chat-token';
const NO_MODELS = 'No model is registered yet: an admin adds one under Settings → Model server & API keys.';

const state = {
  config: { auth_required: false },
  noModels: false,     // cedar has no model registered: messages can't be sent yet
  conversations: [],
  current: null,     // the open conversation, or null on the "new chat" screen
  views: new Map(),  // turn id -> view of the open conversation
  activeTurn: null,  // view whose run is still going, if any
  sending: false,
  loadToken: 0,      // bumps on navigation so stale loads are dropped
};

// -- small helpers -----------------------------------------------------------------------------

const ICONS = {
  terminal: '<svg viewBox="0 0 24 24"><path d="M4 17l6-5-6-5M12 19h8"/></svg>',
  file: '<svg viewBox="0 0 24 24"><path d="M14 3H6v18h12V7zM14 3v4h4"/></svg>',
  pencil: '<svg viewBox="0 0 24 24"><path d="M4 20h4L19 9l-4-4L4 16zM13 7l4 4"/></svg>',
  search: '<svg viewBox="0 0 24 24"><circle cx="11" cy="11" r="6"/><path d="M16 16l4 4"/></svg>',
  tool: '<svg viewBox="0 0 24 24"><path d="M14.7 6.3a4 4 0 0 0-5.4 5.4L3 18l3 3 6.3-6.3a4 4 0 0 0 5.4-5.4l-2.5 2.5-2.4-.6-.6-2.4z"/></svg>',
  chevron: '<svg class="chev" viewBox="0 0 24 24"><path d="M9 6l6 6-6 6"/></svg>',
  check: '<svg viewBox="0 0 24 24"><path d="M5 12.5l4.5 4.5L19 7.5"/></svg>',
  cross: '<svg viewBox="0 0 24 24"><path d="M6 6l12 12M18 6L6 18"/></svg>',
  copy: '<svg viewBox="0 0 24 24"><rect x="8" y="8" width="12" height="12" rx="2"/><path d="M16 8V5a1 1 0 0 0-1-1H5a1 1 0 0 0-1 1v10a1 1 0 0 0 1 1h3"/></svg>',
  trash: '<svg viewBox="0 0 24 24"><path d="M4 7h16M9 7V4h6v3M6 7l1 13h10l1-13"/></svg>',
  logo: '<picture><source media="(prefers-color-scheme: dark)" srcset="/assets/cedar-mark-on-dark.svg"><img class="logo" src="/assets/cedar-mark.svg" alt="" width="22" height="22"></picture>',
  download: '<svg viewBox="0 0 24 24"><path d="M12 4v11M7 10l5 5 5-5M5 19h14"/></svg>',
  eye: '<svg viewBox="0 0 24 24"><path d="M2 12s3.6-6.5 10-6.5S22 12 22 12s-3.6 6.5-10 6.5S2 12 2 12z"/><circle cx="12" cy="12" r="3"/></svg>',
  document: '<svg viewBox="0 0 24 24"><path d="M14 3H6v18h12V7zM14 3v4h4M9 12h6M9 16h6"/></svg>',
  code: '<svg viewBox="0 0 24 24"><path d="M8 7l-5 5 5 5M16 7l5 5-5 5"/></svg>',
  chart: '<svg viewBox="0 0 24 24"><path d="M4 4v16h16M8 16v-4M12 16V8M16 16v-6"/></svg>',
  globe: '<svg viewBox="0 0 24 24"><circle cx="12" cy="12" r="9"/><path d="M3 12h18M12 3c3 3 3 15 0 18M12 3c-3 3-3 15 0 18"/></svg>',
};

function escapeHtml(s) {
  return String(s).replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c]);
}

function h(tag, className, html) {
  const el = document.createElement(tag);
  if (className) el.className = className;
  if (html !== undefined) el.innerHTML = html;
  return el;
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

let toastTimer;
function toast(message) {
  els.toast.textContent = message;
  els.toast.classList.add('show');
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => els.toast.classList.remove('show'), 4000);
}

function storage(op, value) {
  try {
    if (op === 'get') return localStorage.getItem(TOKEN_KEY);
    if (op === 'set') localStorage.setItem(TOKEN_KEY, value);
    if (op === 'clear') localStorage.removeItem(TOKEN_KEY);
  } catch { /* storage may be unavailable; the token then lasts for this page only */ }
  return null;
}

let memoryToken = storage('get');

async function copyText(text, button) {
  try {
    await navigator.clipboard.writeText(text);
    if (button) {
      const old = button.innerHTML;
      button.innerHTML = `${ICONS.check}Copied`;
      setTimeout(() => { button.innerHTML = old; }, 1500);
    }
  } catch {
    toast('Could not copy to the clipboard');
  }
}

// -- markdown ----------------------------------------------------------------------------------
// A small renderer for what models usually write. All text is escaped before any markup is added.

function renderInline(text) {
  const slots = [];
  const stash = (html) => `\u0000${slots.push(html) - 1}\u0000`;
  const link = (href, label) => `<a href="${escapeHtml(href)}" target="_blank" rel="noopener noreferrer">${label}</a>`;

  let s = text.replace(/`([^`\n]+)`/g, (_, code) => stash(`<code>${escapeHtml(code)}</code>`));
  // ![caption](chart.png) for a workspace image becomes a placeholder that image-view.js fills in.
  s = s.replace(/!\[([^\]\n]*)\]\(([^)\s]+)\)/g, (m, alt, href) => {
    const image = cedarImages.markdown(alt, href);
    return image ? stash(image) : m;
  });
  s = s.replace(/\[([^\]\n]+)\]\(([^)\s]+)\)/g, (m, label, href) =>
    /^(https?:|mailto:)/i.test(href) ? stash(link(href, escapeHtml(label))) : m);
  s = s.replace(/\bhttps?:\/\/[^\s<>()]+[^\s<>().,;:!?'"*_]/g, (url) => stash(link(url, escapeHtml(url))));
  s = escapeHtml(s);
  s = s.replace(/\*\*(?=\S)([^*]+?)\*\*/g, '<strong>$1</strong>').replace(/__(?=\S)([^_]+?)__/g, '<strong>$1</strong>');
  s = s.replace(/(^|[^*\w])\*(?=\S)([^*\n]+?)\*(?!\w)/g, '$1<em>$2</em>');
  s = s.replace(/(^|[^_\w])_(?=\S)([^_\n]+?)_(?!\w)/g, '$1<em>$2</em>');
  s = s.replace(/~~(?=\S)([^~]+?)~~/g, '<del>$1</del>');
  return s.replace(/\u0000(\d+)\u0000/g, (_, i) => slots[Number(i)]);
}

const RE = {
  fence: /^\s*(`{3,}|~{3,})\s*([\w+#.-]*)/,
  heading: /^(#{1,6})\s+(.*?)\s*#*\s*$/,
  hr: /^\s*([-*_])(\s*\1){2,}\s*$/,
  quote: /^\s*>/,
  item: /^(\s*)([-*+]|\d{1,9}[.)])\s+(.*)$/,
  tableSep: /^\s*\|?\s*:?-{2,}:?\s*(\|\s*:?-{2,}:?\s*)*\|?\s*$/,
};

const indentOf = (line) => line.match(/^\s*/)[0].replace(/\t/g, '    ').length;
const isTableStart = (lines, i) => lines[i].includes('|') && i + 1 < lines.length && RE.tableSep.test(lines[i + 1]);
const isBlockStart = (lines, i) => {
  const l = lines[i];
  return RE.fence.test(l) || RE.heading.test(l) || RE.hr.test(l) || RE.quote.test(l) || RE.item.test(l) || isTableStart(lines, i);
};

function splitRow(line) {
  let s = line.trim();
  if (s.startsWith('|')) s = s.slice(1);
  if (s.endsWith('|') && !s.endsWith('\\|')) s = s.slice(0, -1);
  return s.split(/(?<!\\)\|/).map((c) => c.trim().replace(/\\\|/g, '|'));
}

function codeBlock(code, lang) {
  return `<div class="code"><div class="code-head"><span>${escapeHtml(lang || 'text')}</span>` +
    `<button type="button" class="copy-btn" data-copy-code>${ICONS.copy}Copy</button></div>` +
    `<pre><code>${escapeHtml(code)}</code></pre></div>`;
}

function parseList(lines, start) {
  const base = indentOf(lines[start]);
  const ordered = /^\d/.test(lines[start].trim());
  const startNum = ordered ? parseInt(lines[start].trim(), 10) : 1;
  const items = [];
  let i = start;
  while (i < lines.length) {
    const line = lines[i];
    if (!line.trim()) {
      let j = i + 1;
      while (j < lines.length && !lines[j].trim()) j++;
      if (j < lines.length && indentOf(lines[j]) >= base && (RE.item.test(lines[j]) || indentOf(lines[j]) > base)) {
        if (indentOf(lines[j]) > base) items[items.length - 1].rest.push('');
        i = j;
        continue;
      }
      break;
    }
    const ind = indentOf(line);
    const m = line.match(RE.item);
    if (m && ind === base) {
      items.push({ text: [m[3]], rest: [] });
    } else if (ind > base && items.length) {
      const item = items[items.length - 1];
      if (!item.rest.length && !RE.item.test(line) && !RE.fence.test(line)) item.text.push(line.trim());
      else item.rest.push(line.slice(Math.min(ind, base + 2)));
    } else if (!m && ind <= base && items.length && !isBlockStart(lines, i)) {
      items[items.length - 1].text.push(line.trim()); // lazy continuation
    } else {
      break;
    }
    i++;
  }
  const body = items.map((it) => {
    const rest = it.rest.length ? renderMarkdown(it.rest.join('\n')) : '';
    return `<li>${renderInline(it.text.join(' '))}${rest}</li>`;
  }).join('');
  const html = ordered ? `<ol${startNum !== 1 ? ` start="${startNum}"` : ''}>${body}</ol>` : `<ul>${body}</ul>`;
  return [html, i];
}

function renderTable(lines, start) {
  const head = splitRow(lines[start]);
  const align = splitRow(lines[start + 1]).map((c) =>
    c.startsWith(':') && c.endsWith(':') ? 'center' : c.endsWith(':') ? 'right' : '');
  const cell = (tag, text, k) => `<${tag}${align[k] ? ` style="text-align:${align[k]}"` : ''}>${renderInline(text)}</${tag}>`;
  let i = start + 2;
  const rows = [];
  while (i < lines.length && lines[i].trim() && lines[i].includes('|')) {
    const cells = splitRow(lines[i++]);
    rows.push(`<tr>${head.map((_, k) => cell('td', cells[k] ?? '', k)).join('')}</tr>`);
  }
  const html = `<div class="table-wrap"><table><thead><tr>${head.map((t, k) => cell('th', t, k)).join('')}</tr></thead>` +
    `<tbody>${rows.join('')}</tbody></table></div>`;
  return [html, i];
}

function renderMarkdown(src) {
  const lines = String(src).replace(/\u0000/g, '').replace(/\r\n?/g, '\n').split('\n');
  const out = [];
  let i = 0;
  while (i < lines.length) {
    const line = lines[i];
    let m;
    if (!line.trim()) { i++; continue; }
    if ((m = line.match(RE.fence))) {
      const close = new RegExp(`^\\s*${m[1][0] === '`' ? '`' : '~'}{${m[1].length},}\\s*$`);
      const code = [];
      const pad = indentOf(line);
      for (i++; i < lines.length && !close.test(lines[i]); i++) code.push(lines[i].slice(Math.min(pad, indentOf(lines[i]))));
      i++;
      out.push(codeBlock(code.join('\n'), m[2]));
    } else if ((m = line.match(RE.heading))) {
      const n = m[1].length;
      out.push(`<h${n}>${renderInline(m[2])}</h${n}>`);
      i++;
    } else if (RE.hr.test(line)) {
      out.push('<hr>');
      i++;
    } else if (RE.quote.test(line)) {
      const quoted = [];
      while (i < lines.length && RE.quote.test(lines[i])) quoted.push(lines[i++].replace(/^\s*> ?/, ''));
      out.push(`<blockquote>${renderMarkdown(quoted.join('\n'))}</blockquote>`);
    } else if (isTableStart(lines, i)) {
      const [html, next] = renderTable(lines, i);
      out.push(html);
      i = next;
    } else if (RE.item.test(line)) {
      const [html, next] = parseList(lines, i);
      out.push(html);
      i = next;
    } else {
      const para = [lines[i++].trim()];
      while (i < lines.length && lines[i].trim() && !isBlockStart(lines, i)) para.push(lines[i++].trim());
      out.push(`<p>${para.map(renderInline).join('<br>')}</p>`);
    }
  }
  return out.join('');
}

// -- API ---------------------------------------------------------------------------------------

let tokenPrompt = null;

/** Ask for the access token. Concurrent callers share one dialog. */
function askToken(rejected) {
  if (!tokenPrompt) {
    tokenPrompt = new Promise((resolve) => {
      els.tokenMessage.textContent = rejected
        ? 'That token was not accepted. Enter the access token for this server.'
        : 'This server requires an access token.';
      els.tokenInput.value = '';
      els.tokenForm.onsubmit = () => {
        memoryToken = els.tokenInput.value.trim();
        storage('set', memoryToken);
        tokenPrompt = null;
        resolve();
      };
      els.tokenDialog.addEventListener('cancel', (e) => e.preventDefault(), { once: true });
      els.tokenDialog.showModal();
    });
  }
  return tokenPrompt;
}

function authHeaders() {
  return memoryToken ? { Authorization: `Bearer ${memoryToken}` } : {};
}

async function api(method, path, body) {
  const headers = authHeaders();
  if (body !== undefined) headers['Content-Type'] = 'application/json';
  const res = await fetch(`/api${path}`, { method, headers, body: body === undefined ? undefined : JSON.stringify(body) });
  if (res.status === 401) {
    await askToken(true);
    return api(method, path, body);
  }
  if (res.status === 204) return null;
  const data = await res.json().catch(() => null);
  if (!res.ok) throw new Error(data?.error?.message || `Request failed (${res.status})`);
  return data;
}

// -- sidebar -----------------------------------------------------------------------------------

async function loadConversations() {
  try {
    state.conversations = (await api('GET', '/conversations')).conversations;
  } catch (e) {
    toast(e.message);
  }
  renderSidebar();
}

function renderSidebar() {
  els.convList.textContent = '';
  if (!state.conversations.length) {
    els.convList.append(h('div', 'empty-list', 'No conversations yet'));
    return;
  }
  for (const c of state.conversations) {
    const a = h('a', 'conv');
    a.href = `#/c/${c.id}`;
    if (state.current?.id === c.id) a.classList.add('active');
    const name = h('span', 'name');
    name.textContent = c.title || 'New chat';
    a.append(name);
    a.classList.toggle('is-unread', Boolean(c.unread));
    if (state.current?.id === c.id && state.activeTurn) a.append(h('span', 'live'));
    else if (c.needs_answer) a.append(h('span', 'needs', 'Needs answer'));
    else if (c.unread) a.append(h('span', 'unread'));
    const del = h('button', 'del', ICONS.trash);
    del.type = 'button';
    del.title = 'Delete';
    del.setAttribute('aria-label', `Delete ${c.title || 'conversation'}`);
    del.addEventListener('click', (e) => { e.preventDefault(); e.stopPropagation(); deleteConversation(c); });
    a.append(del);
    a.addEventListener('click', () => toggleSidebar(false));
    els.convList.append(a);
  }
}

function toggleSidebar(open) {
  els.sidebar.classList.toggle('open', open);
  els.backdrop.classList.toggle('open', open);
}

async function deleteConversation(c) {
  if (!(await cedarConfirm(`Delete "${c.title || 'New chat'}"? This cannot be undone.`, { ok: 'Delete' }))) return;
  try {
    await api('DELETE', `/conversations/${c.id}`);
  } catch (e) {
    toast(e.message);
    return;
  }
  state.conversations = state.conversations.filter((x) => x.id !== c.id);
  if (state.current?.id === c.id) location.hash = '#/';
  renderSidebar();
}

// -- conversation view -------------------------------------------------------------------------

function resetView() {
  for (const view of state.views.values()) view.abort.abort();
  state.views.clear();
  state.activeTurn = null;
  els.thread.textContent = '';
  if (els.filesDialog.open) els.filesDialog.close();
}

function setHeader() {
  const c = state.current;
  els.main.classList.toggle('empty', !c);
  els.title.textContent = c ? (c.title || 'New chat') : '';
  els.deleteChat.hidden = !c;
  els.filesBtn.hidden = !c;
  els.scheduleBadge.hidden = !c?.schedule_id;
  cedarStar.setCurrent(c);
  cedarNotify.setTitle(c?.title ? `${c.title} – Cedar` : 'Cedar');
}

/** The open conversation has been looked at: nothing in it is unread any more. */
function markSeen() {
  const c = state.current;
  if (!c || document.visibilityState !== 'visible') return;
  c.unread = false;
  const listed = state.conversations.find((x) => x.id === c.id);
  if (listed) listed.unread = false;
  cedarNotify.seen(c.id);
}

/** Whether the open conversation has a finished response to schedule. */
function hasCompletedTurn() {
  for (const view of state.views.values()) {
    if (view.lastEvent === 'run.completed' || (!view.lastEvent && view.turn.status === 'completed')) return true;
  }
  return false;
}

function showNewChat() {
  state.loadToken++;
  resetView();
  state.current = null;
  cedarSettings.conversationChanged();
  cedarAttach.clear();
  setHeader();
  const hour = new Date().getHours();
  els.greeting.textContent = hour < 5 ? 'Up late? How can I help?' : hour < 12 ? 'Good morning. How can I help?'
    : hour < 18 ? 'How can I help you today?' : 'Good evening. How can I help?';
  renderSidebar();
  updateComposer();
  els.input.focus();
}

async function openConversation(id) {
  const token = ++state.loadToken;
  resetView();
  let detail;
  try {
    detail = await api('GET', `/conversations/${id}`);
  } catch (e) {
    if (token !== state.loadToken) return;
    toast(e.message);
    location.hash = '#/';
    return;
  }
  if (token !== state.loadToken) return;
  state.current = detail.conversation;
  cedarSettings.conversationChanged();
  cedarAttach.clear();
  setHeader();
  restorePickers(detail.conversation);
  for (const turn of detail.turns) addTurn(turn);
  renderSidebar();
  updateComposer();
  scrollToBottom(true);
  els.input.focus();
  markSeen();
}

// -- the model and reasoning pickers -----------------------------------------------------------

/** Load cedar's registry into the pickers. Without a model registered, sending is off. */
async function loadModels() {
  try { await cedarModels.load(api); } catch { return; } // cedar unreachable: leave the pickers as they are
  state.noModels = !cedarModels.all().length;
  const model = cedarModels.byId(els.model.value) ? els.model.value : '';
  cedarModels.fillSelect(els.model, { selected: model });
  cedarModels.fillLevels(els.reasoning, els.model.value, els.reasoning.value);
  syncAttach();
  sendMenu?.refresh();
  updateComposer();
}

/** The paperclip takes PDFs for any model, and images while the chosen one (or the default) does. */
function syncAttach() {
  cedarAttach.setImages(cedarModels.multimodal(els.model.value));
}

/** Put the conversation's last model and reasoning level back into the pickers. */
function restorePickers(conversation) {
  if (!cedarModels.loaded()) return;
  const model = cedarModels.byId(conversation.model) ? conversation.model : '';
  cedarModels.fillSelect(els.model, { selected: model });
  cedarModels.fillLevels(els.reasoning, model, conversation.reasoning || '');
  syncAttach();
  sendMenu?.refresh();
}

cedarModels.bindPair(els.model, els.reasoning);
els.model.addEventListener('change', syncAttach);
// The send button's menu: shows the chosen model and level, and changes them.
let sendMenu = null;
document.addEventListener('cedar-models-changed', loadModels);

function route() {
  const m = location.hash.match(/^#\/c\/([0-9a-f-]{36})$/i);
  if (m) {
    if (state.current?.id !== m[1]) openConversation(m[1]);
  } else {
    showNewChat();
  }
}

function nearBottom() {
  const s = els.scroll;
  return s.scrollHeight - s.scrollTop - s.clientHeight < 120;
}

function scrollToBottom(force) {
  if (force || nearBottom()) els.scroll.scrollTop = els.scroll.scrollHeight;
}

// -- turns -------------------------------------------------------------------------------------

/** Render a turn and start following its run's events. */
function addTurn(turn) {
  const el = h('div', 'turn');
  const user = h('div', 'user-msg');
  const bubble = h('div', 'bubble');
  if (turn.attachments?.length) bubble.append(cedarAttach.chips(turn.attachments));
  bubble.append(document.createTextNode(turn.prompt));
  user.append(bubble);
  const body = h('div', 'assistant');
  const thinking = h('div', 'thinking', `${ICONS.logo}<span>Thinking…</span>`);
  body.append(thinking);
  el.append(user, body);
  cedarClarify.settle(els.thread); // this message answers any question still open above it
  els.thread.append(el);

  const view = {
    turn, el, body, thinking,
    tools: new Map(), texts: [], images: [], usage: { input: 0, output: 0 },
    abort: new AbortController(), done: false,
    calls: [],            // {id, name, input, output?, is_error?, sources?}, for the sources
    mds: [],              // Cedar's text blocks, for linking [n]
    started: null,        // run.started, for the sandbox line
    planEl: null, planData: null,
    approvals: new Map(), // approval id -> its card
    approvalsGranted: 0,
    reasoningEl: null,    // the fold reasoning is being added to
  };
  state.views.set(turn.id, view);
  if (ACTIVE.has(turn.status)) {
    state.activeTurn = view;
    updateComposer();
  }
  followEvents(view);
  return view;
}

/** Insert content above the "thinking" indicator so it stays last while the run is going. */
function place(view, node) {
  if (view.thinking.isConnected) view.body.insertBefore(node, view.thinking);
  else view.body.append(node);
  scrollToBottom(false);
}

function parseSse(raw) {
  let data = '';
  for (const line of raw.split('\n')) {
    if (line.startsWith('data:')) data += line.slice(line[5] === ' ' ? 6 : 5);
  }
  if (!data) return null; // keep-alive comment
  try { return JSON.parse(data); } catch { return null; }
}

/** Read the turn's events (replayed from the start, then live) until a terminal event. */
async function followEvents(view) {
  let after = 0;
  let failures = 0;
  while (!view.done) {
    try {
      const res = await fetch(`/api/turns/${view.turn.id}/events?after=${after}`, {
        headers: authHeaders(), signal: view.abort.signal,
      });
      if (res.status === 401) { await askToken(true); continue; }
      if (!res.ok) {
        const data = await res.json().catch(() => null);
        const err = new Error(data?.error?.message || `Could not load this response (${res.status})`);
        err.fatal = res.status < 500;
        throw err;
      }
      const reader = res.body.getReader();
      const decoder = new TextDecoder();
      let buf = '';
      for (;;) {
        const { value, done } = await reader.read();
        if (done) break;
        buf += decoder.decode(value, { stream: true });
        let cut;
        while ((cut = buf.indexOf('\n\n')) >= 0) {
          const event = parseSse(buf.slice(0, cut));
          buf = buf.slice(cut + 2);
          if (!event) continue;
          after = event.seq;
          failures = 0;
          applyEvent(view, event);
        }
      }
    } catch (e) {
      if (view.abort.signal.aborted) return;
      if (e.fatal || ++failures > 5) {
        finishTurn(view, { type: 'run.failed', data: { error: e.message } });
        return;
      }
    }
    if (!view.done) await sleep(Math.min(1000 * 2 ** failures, 8000));
  }
}

// -- the plan, approvals, reasoning and sources ------------------------------------------------

/** What the indicator under a running reply says. */
function setThinking(view, text) {
  const label = view.thinking.querySelector('span');
  if (label) label.textContent = text;
  view.thinking.classList.toggle('waiting', text.startsWith('Waiting'));
}

const sourcesOf = (view) => cedarTrust.sources.collect(view.calls);

/** The plan Cedar sent: editable while the run waits on it, a checklist once it is going. */
function showPlan(view, data, awaiting) {
  const plan = cedarTrust.plan.parse(data);
  if (!plan) return;
  view.planData = data;
  // Only a turn still going can be answered (a finished one is replayed when a chat opens).
  const waits = awaiting && !view.done && ACTIVE.has(view.turn.status);
  const el = cedarTrust.plan.element(plan, {
    awaiting: waits,
    note: waits ? cedarTrust.plan.awaitingNote(plan) : data.by === 'user' ? 'Agreed' : data.by === 'timeout' ? 'No answer, so Cedar went ahead' : '',
    onApprove: async (steps) => {
      try { await api('POST', `/turns/${view.turn.id}/plan`, { steps }); } catch (e) { toast(e.message); throw e; }
      setThinking(view, 'Thinking…');
    },
  });
  if (view.planEl) view.planEl.replaceWith(el); else place(view, el);
  view.planEl = el;
  if (waits) setThinking(view, 'Waiting for your go on the plan');
}

/** A risky call waits on the reader: the card with Allow once / Always / Skip. */
function showApproval(view, d) {
  const spec = { id: d.id, tool: d.tool, input: d.input || {}, reason: d.reason, scope: d.scope, rule: d.rule };
  const el = cedarTrust.approval.element(spec, {
    onDecide: async (decision) => {
      try { await api('POST', `/turns/${view.turn.id}/approvals/${encodeURIComponent(spec.id)}`, { decision }); }
      catch (e) { toast(e.message); throw e; }
      setThinking(view, 'Thinking…');
    },
  });
  if (view.done || !ACTIVE.has(view.turn.status)) el.querySelector('.cedar-approval-actions').hidden = true;
  view.approvals.set(String(d.id), el);
  place(view, el);
  setThinking(view, 'Waiting for your OK');
}

/** Cedar's reasoning, folded; text for the same stretch of thinking is added to one fold. */
function showReasoning(view, d) {
  if (!view.reasoningEl) {
    view.reasoningEl = cedarTrust.reasoning.element(d.text || '', d.duration_ms);
    place(view, view.reasoningEl);
  } else {
    view.reasoningEl.cedarReasoning.append(d.text || '');
    if (typeof d.duration_ms === 'number') view.reasoningEl.cedarReasoning.done(d.duration_ms);
  }
  if (d.done) view.reasoningEl = null;
}

function applyEvent(view, event) {
  const d = event.data || {};
  switch (event.type) {
    case 'run.started': view.started = d; break;
    case 'plan': showPlan(view, d, d.awaiting === true); break;
    case 'plan.updated': showPlan(view, d, false); setThinking(view, 'Thinking…'); break;
    case 'assistant.reasoning': showReasoning(view, d); break;
    case 'approval.required': showApproval(view, d); break;
    case 'approval.resolved': {
      const el = view.approvals.get(String(d.id));
      if (el && el.dataset.state === 'pending') {
        cedarTrust.approval.settle(el, d.decision, event.created_at);
        el.querySelector('.cedar-approval-actions').hidden = false;
      }
      if (d.decision === 'allow' || d.decision === 'allow_always') view.approvalsGranted++;
      setThinking(view, 'Thinking…');
      break;
    }
    case 'assistant.text': {
      view.texts.push(d.text);
      view.reasoningEl = null;
      const md = h('div', 'md', renderMarkdown(d.text));
      cedarImages.hydrate(md, { onLoad: () => scrollToBottom(false) });
      view.mds.push(md);
      const sources = sourcesOf(view);
      if (sources.length) cedarTrust.sources.cite(md, sources);
      place(view, md);
      break;
    }
    case 'tool.call':
      view.calls.push({ id: d.id, name: d.name, input: d.input || {}, stepId: d.step ?? null });
      view.reasoningEl = null;
      if (d.step != null) view.planEl?.cedarPlan.mark(d.step, 'running');
      place(view, toolCall(view, d));
      break;
    case 'tool.result': {
      const call = view.calls.find((c) => c.id === d.id);
      if (call) {
        Object.assign(call, { output: String(d.output ?? ''), is_error: Boolean(d.is_error), sources: d.sources });
        if (call.stepId != null) view.planEl?.cedarPlan.mark(call.stepId, call.is_error ? 'error' : 'done');
      }
      toolResult(view, d);
      break;
    }
    case 'usage':
      view.usage.input += d.input_tokens || 0;
      view.usage.output += d.output_tokens || 0;
      if (typeof d.cost_usd === 'number') view.usage.cost_usd = (view.usage.cost_usd || 0) + d.cost_usd;
      break;
    default:
      if (TERMINAL.has(event.type)) finishTurn(view, event);
  }
}

// Tools whose `path` input is a file they write, so a finished call gets a download button.
const WRITING_TOOLS = ['write_file', 'generate_pdf', 'generate_chart', 'download_file'];

const TOOL_ICONS = {
  bash: 'terminal', read_file: 'file', write_file: 'pencil', web_search: 'search',
  generate_pdf: 'document', read_document: 'document', run_python: 'code',
  generate_chart: 'chart', fetch_url: 'globe', download_file: 'download', weather_forecast: 'globe',
  show_map: 'globe', show_recipe: 'document', plan_trip: 'globe', compare_stocks: 'chart',
  show_map: 'globe', show_recipe: 'document', plan_trip: 'globe', ask_user: 'tool',
  search_products: 'search',
};

function toolSummary(name, input) {
  if (!input || typeof input !== 'object') return '';
  if (typeof input.command === 'string') return input.command.split('\n')[0];
  if (name === 'show_recipe' && typeof input.title === 'string') return input.title;
  if (name === 'compare_stocks') return stocksSubject(input);
  if (name === 'ask_user' && Array.isArray(input.questions)) {
    return input.questions.map((q) => q?.header || q?.question).filter(Boolean).join(' · ');
  }
  if (name === 'run_python' && typeof input.code === 'string') {
    return input.code.split('\n').find((l) => l.trim()) || '';
  }
  if (typeof input.url === 'string') return input.path ? `${input.url} → ${input.path}` : input.url;
  if (typeof input.path === 'string') return input.path;
  if (typeof input.query === 'string') return input.query;
  if (typeof input.location === 'string') return input.location;
  if (name === 'plan_trip') return tripSubject(input);
  if (Array.isArray(input.markers)) {
    return input.title || input.markers.map((m) => m?.location || m?.label || `${m?.latitude}, ${m?.longitude}`).join(' · ');
  }
  return JSON.stringify(input);
}

/** What a plan_trip call is about: its title, else "from → to". */
function tripSubject(input) {
  if (input.title) return input.title;
  const end = (e) => (typeof e === 'string' ? e : e?.label || e?.location || (e ? `${e.latitude}, ${e.longitude}` : '?'));
  return `${end(input.from)} → ${end(input.to)}`;
}

/** What a compare_stocks call is about: its symbols. */
function stocksSubject(input) {
  const symbols = Array.isArray(input.symbols) ? input.symbols : [input.symbols];
  return symbols.filter((s) => typeof s === 'string').join(' · ');
}

function toolCall(view, d) {
  const icon = ICONS[TOOL_ICONS[d.name]] || ICONS.tool;
  const details = h('details', 'tool running');
  details.dataset.tool = d.name;
  const summary = h('summary', '', `${icon}<span class="tool-name"></span><span class="tool-arg"></span>` +
    `<span class="meta"></span><span class="state"><span class="spinner"></span></span>${ICONS.chevron}`);
  summary.querySelector('.tool-name').textContent = d.name;
  summary.querySelector('.tool-arg').textContent = toolSummary(d.name, d.input);
  const body = h('div', 'tool-body');
  const input = d.input || {};
  let shown;
  if (d.name === 'bash' && typeof input.command === 'string') shown = `$ ${input.command}`;
  else if (d.name === 'run_python' && typeof input.code === 'string') shown = input.code;
  else if ((d.name === 'write_file' || d.name === 'generate_pdf') && typeof input.content === 'string') shown = `${input.path}\n\n${input.content}`;
  else shown = JSON.stringify(input, null, 2);
  const inputPre = h('pre');
  inputPre.textContent = shown;
  body.append(h('div', 'label', 'Input'), inputPre);
  details.append(summary, body);
  if (WRITING_TOOLS.includes(d.name) && typeof input.path === 'string') details.dataset.path = input.path;
  details.cedarInput = input; // for the images the result may point at
  view.tools.set(d.id, details);
  return details;
}

function toolResult(view, d) {
  const details = view.tools.get(d.id);
  if (!details) return;
  details.classList.remove('running');
  details.classList.add(d.is_error ? 'err' : 'ok');
  details.querySelector('.state').innerHTML = d.is_error ? ICONS.cross : ICONS.check;
  if (typeof d.duration_ms === 'number') {
    details.querySelector('.meta').textContent = d.duration_ms < 1000 ? `${d.duration_ms} ms` : `${(d.duration_ms / 1000).toFixed(1)} s`;
  }
  const out = h('pre', 'output');
  const text = String(d.output ?? '');
  out.textContent = text.trim() ? text : '(no output)';
  const label = h('div', 'label', d.is_error ? 'Error' : 'Output');
  const written = !d.is_error && workspacePath(details.dataset.path);
  if (written) {
    label.classList.add('with-action');
    const actions = h('span', 'label-actions');
    const view = viewButton(written);
    if (view) actions.append(view);
    actions.append(downloadButton(written));
    label.append(actions);
  }
  details.querySelector('.tool-body').append(label, out);
  // A map is the point of show_map, a recipe card of show_recipe, the stock card of
  // compare_stocks, the product card of search_products, and the chart card of generate_chart
  // (drawn from the call's data), so each sits under the call instead of inside the collapsed
  // details.
  const card = { show_map: cedarMap, show_recipe: cedarRecipe, compare_stocks: cedarStocks, search_products: cedarProducts, generate_chart: cedarChart }[details.dataset.tool];
  let carded = false;
  if (card && !d.is_error) {
    const spec = card.parse(text, details.cedarInput);
    if (spec) {
      details.after(card.element(spec));
      carded = true;
      scrollToBottom(false);
    }
  }
  // Any other tool may print charts (`[chart] {spec}` lines): they are drawn under the call, and
  // its output keeps the rest.
  if (!card && !d.is_error) {
    const found = cedarChart.fromOutput(text);
    if (found.charts.length) {
      out.textContent = found.text || '(no other output)';
      details.after(...found.charts.map((spec) => cedarChart.element(spec)));
      scrollToBottom(false);
    }
  }
  // The same goes for a plan_trip result, drawn as the trip's routes and options.
  if (details.dataset.tool === 'plan_trip' && !d.is_error) {
    const spec = cedarTransit.parse(text);
    if (spec) {
      details.after(cedarTransit.element(spec));
      scrollToBottom(false);
    }
  }
  // And an ask_user result, drawn as a form whose answer is sent as the next message. A later
  // message has already answered it.
  if (details.dataset.tool === 'ask_user' && !d.is_error) {
    const spec = cedarClarify.parse(text);
    if (spec) {
      const answered = view.el.nextElementSibling !== null;
      details.after(cedarClarify.element(spec, { answered, onSubmit: answerQuestions }));
      scrollToBottom(false);
    }
  }
  // The images a call produced (a picture it saved, or one a command or script wrote) are shown
  // with the reply, once each: see finishTurn. A chart drawn as a card isn't shown again.
  if (!d.is_error && !carded) {
    for (const path of cedarImages.produced(details.dataset.tool, details.cedarInput, text)) {
      if (!view.images.includes(path)) view.images.push(path);
    }
  }
}

/** Shown when the server stopped the reply at its output limit (`stop_reason: max_tokens`). */
const CUT_OFF =
  "The reply was cut off at the model's output limit. A model that thinks before answering can use all of it before writing any text. An admin can raise it under Settings → Server, as the model's Max tokens per reply.";

function finishTurn(view, event) {
  if (view.done) return;
  view.done = true;
  view.thinking.remove();
  const d = event.data || {};
  const figures = cedarImages.figuresFor(view.images, { skip: cedarImages.shown(view.body), onLoad: () => scrollToBottom(false) });
  if (figures.length) view.body.append(...figures);
  if (event.type === 'run.failed') {
    const box = h('div', 'notice error');
    box.textContent = d.error || 'Something went wrong.';
    view.body.append(box);
  } else if (event.type === 'run.cancelled') {
    const note = h('div', 'notice muted');
    note.textContent = d.reason === 'server shutting down' ? 'Stopped: the server shut down.' : 'Stopped.';
    view.body.append(note);
  } else if (d.stop_reason === 'max_tokens') {
    view.body.append(h('div', 'notice error', CUT_OFF));
  } else if (!view.texts.length && d.stop_reason !== 'ask_user') {
    view.body.append(h('div', 'notice muted', 'No text in this response.'));
  }
  for (const tool of view.tools.values()) {
    if (tool.classList.contains('running')) {
      tool.classList.replace('running', 'err');
      tool.querySelector('.state').innerHTML = ICONS.cross;
    }
  }
  // A plan or approval still waiting when the run ended can no longer be answered.
  if (view.planEl?.dataset.state === 'awaiting') showPlan(view, view.planData, false);
  view.planEl?.cedarPlan?.finish(event.type === 'run.completed');
  for (const el of view.approvals.values()) {
    if (el.dataset.state === 'pending') el.querySelector('.cedar-approval-actions').hidden = true;
  }
  const sources = sourcesOf(view);
  if (sources.length) {
    for (const md of view.mds) cedarTrust.sources.cite(md, sources);
    const box = h('details', 'sources');
    box.append(h('summary', '', `${ICONS.globe}<span>Sources · ${sources.length}</span>${ICONS.chevron}`));
    box.append(cedarTrust.sources.element(sources, { onOpen: (s) => s.path && downloadFile(workspacePath(s.path)) }));
    view.body.append(box);
  }

  const footer = h('div', 'turn-footer');
  if (view.texts.length) {
    const copy = h('button', 'copy-btn', `${ICONS.copy}Copy`);
    copy.type = 'button';
    copy.addEventListener('click', () => copyText(view.texts.join('\n\n'), copy));
    footer.append(copy);
  }
  if (view.usage.input || view.usage.output) {
    const tokens = h('span', 'tokens');
    const sandbox = view.started?.sandbox ? ` · ${cedarTrust.sandbox.text(view.started, view.approvalsGranted)}` : '';
    tokens.textContent = `${cedarTrust.cost.text(view.usage)}${sandbox}`;
    footer.append(tokens);
  }
  if (footer.childElementCount) view.body.append(footer);
  view.lastEvent = event.type;

  if (state.activeTurn === view) {
    state.activeTurn = null;
    renderSidebar();
    // Watched to the end: not news. Hidden tabs leave it unread, for the inbox and the title.
    markSeen();
  }
  updateComposer();
  if (els.filesDialog.open) loadFiles(); // the response may have added or changed files
  scrollToBottom(false);
}

// -- workspace files ---------------------------------------------------------------------------

/** A path relative to the workspace, or null if `path` points elsewhere. */
function workspacePath(path) {
  if (typeof path !== 'string') return null;
  let p = path.trim();
  if (p.startsWith('/workspace/')) p = p.slice('/workspace/'.length);
  else if (p.startsWith('./')) p = p.slice(2);
  if (!p || p.startsWith('/') || p.split('/').some((part) => !part || part === '.' || part === '..')) return null;
  return p;
}

function formatSize(n) {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(n < 10 * 1024 ? 1 : 0)} KB`;
  return `${(n / (1024 * 1024)).toFixed(1)} MB`;
}

/** API path (without the `/api` prefix) of the conversation's file listing, or of one file. */
function filesPath(path) {
  const base = `/conversations/${state.current.id}/files`;
  return path ? `${base}/${path.split('/').map(encodeURIComponent).join('/')}` : base;
}

function downloadButton(path) {
  const button = h('button', 'copy-btn', `${ICONS.download}Download`);
  button.type = 'button';
  button.title = `Download ${path}`;
  button.addEventListener('click', () => downloadFile(path, button));
  return button;
}

/** A button that previews the file (shared viewer), or null for files it can't show. */
const viewButton = (path) => cedarViewer.viewButton(path, { icon: ICONS.eye });

/** Fetch a workspace file with the access token. Returns the ok response, or throws. */
async function fetchWorkspaceFile(path) {
  if (!state.current) throw new Error('No conversation is open.');
  let res = await fetch(`/api${filesPath(path)}`, { headers: authHeaders() });
  if (res.status === 401) {
    await askToken(true);
    res = await fetch(`/api${filesPath(path)}`, { headers: authHeaders() });
  }
  if (!res.ok) {
    const data = await res.json().catch(() => null);
    throw new Error(data?.error?.message || `Could not fetch the file (${res.status})`);
  }
  return res;
}

/** Fetch the file with the access token and hand it to the browser as a download. */
async function downloadFile(path, button) {
  if (!state.current) return;
  if (button) button.disabled = true;
  try {
    const res = await fetchWorkspaceFile(path);
    const blob = await res.blob();
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = path.split('/').pop();
    document.body.append(a);
    a.click();
    a.remove();
    setTimeout(() => URL.revokeObjectURL(url), 60_000);
  } catch (e) {
    toast(e.message);
  } finally {
    if (button) button.disabled = false;
  }
}

function renderFiles(listing) {
  els.filesList.textContent = '';
  if (!listing.files.length) {
    els.filesList.append(h('div', 'empty-list', 'No files in the workspace yet.'));
    return;
  }
  for (const f of listing.files) {
    const row = h('div', 'file-row');
    const path = h('span', 'path');
    const bdi = document.createElement('bdi');
    bdi.textContent = f.path;
    path.title = f.path;
    path.append(bdi);
    const size = h('span', 'size');
    size.textContent = formatSize(f.size);
    const view = viewButton(f.path);
    row.append(path, size);
    if (view) row.append(view);
    row.append(downloadButton(f.path));
    els.filesList.append(row);
  }
  if (listing.truncated) els.filesList.append(h('div', 'empty-list', 'Only the first 1,000 files are shown.'));
}

async function loadFiles() {
  if (!state.current) return;
  const id = state.current.id;
  els.filesList.textContent = '';
  els.filesList.append(h('div', 'empty-list', 'Loading…'));
  try {
    const listing = await api('GET', filesPath());
    if (state.current?.id !== id || !els.filesDialog.open) return;
    renderFiles(listing);
  } catch (e) {
    if (state.current?.id !== id) return;
    els.filesList.textContent = '';
    const note = h('div', 'empty-list');
    note.textContent = e.message;
    els.filesList.append(note);
  }
}

function openFiles() {
  if (!state.current) return;
  els.filesDialog.showModal();
  loadFiles();
}

// -- composer ----------------------------------------------------------------------------------

function updateComposer() {
  const busy = Boolean(state.activeTurn);
  els.send.classList.toggle('stop', busy);
  els.send.setAttribute('aria-label', busy ? 'Stop response' : 'Send message');
  els.send.disabled = state.sending || state.noModels || (!busy && !els.input.value.trim());
  els.input.placeholder = state.noModels ? NO_MODELS : state.current ? 'Reply to Cedar…' : 'How can I help you today?';
  els.hint.textContent = busy ? 'Working… you can stop the response at any time.' : '';
  els.scheduleBtn.hidden = !state.current || busy || state.sending || !hasCompletedTurn();
}

function autosize() {
  els.input.style.height = 'auto';
  els.input.style.height = `${els.input.scrollHeight}px`;
}

async function stopActive() {
  const view = state.activeTurn;
  if (!view) return;
  els.send.disabled = true;
  try {
    await api('POST', `/turns/${view.turn.id}/cancel`);
  } catch (e) {
    toast(e.message);
    els.send.disabled = false;
  }
}

/** Send the answers from a question form; throws if they could not be sent, so the form stays open. */
async function answerQuestions(text) {
  if (state.activeTurn || state.sending) {
    toast('Wait for the response to finish, or stop it, before answering.');
    throw new Error('busy');
  }
  if (!(await sendMessage(text))) throw new Error('not sent');
}

/**
 * Send the message box's text, or `answer` (from a question form) with the box left as it is.
 * Resolves to whether the message was sent.
 */
async function sendMessage(answer) {
  const typed = typeof answer !== 'string';
  const content = (typed ? els.input.value : answer).trim();
  if (!content || state.sending || state.activeTurn) return false;
  state.sending = true;
  updateComposer();
  const token = state.loadToken;
  try {
    let conversation = state.current;
    if (!conversation) {
      conversation = await api('POST', '/conversations', {});
      state.current = conversation;
      state.conversations.unshift(conversation);
      await cedarSettings.beforeSend(conversation);
      history.pushState(null, '', `#/c/${conversation.id}`);
    }
    const body = { content };
    if (els.model.value) body.model = els.model.value;
    if (els.reasoning.value) body.reasoning = els.reasoning.value;
    const attachments = typed ? cedarAttach.payload() : [];
    if (attachments.length) body.attachments = attachments;
    const turn = await api('POST', `/conversations/${conversation.id}/messages`, body);
    if (typed) {
      els.input.value = '';
      cedarAttach.clear(); // only once sent: a failed send keeps the files, like the text
      autosize();
    }
    if (token !== state.loadToken) return true; // the user navigated away meanwhile
    if (!conversation.title) conversation.title = turn.prompt.split('\n').find((l) => l.trim())?.trim().slice(0, 60);
    state.conversations = [conversation, ...state.conversations.filter((c) => c.id !== conversation.id)];
    setHeader();
    addTurn(turn);
    scrollToBottom(true);
    loadConversations(); // picks up the server-side title
    return true;
  } catch (e) {
    toast(e.message);
    setHeader();
    renderSidebar();
    return false;
  } finally {
    state.sending = false;
    updateComposer();
  }
}

// -- title editing -----------------------------------------------------------------------------

function startRename() {
  if (!state.current) return;
  els.title.hidden = true;
  els.titleInput.hidden = false;
  els.titleInput.value = state.current.title || '';
  els.titleInput.focus();
  els.titleInput.select();
}

async function finishRename(save) {
  if (els.titleInput.hidden) return;
  const title = els.titleInput.value.trim();
  els.titleInput.hidden = true;
  els.title.hidden = false;
  const c = state.current;
  if (!save || !c || !title || title === c.title) return;
  try {
    const updated = await api('PATCH', `/conversations/${c.id}`, { title });
    c.title = updated.title;
    const listed = state.conversations.find((x) => x.id === c.id);
    if (listed) listed.title = updated.title;
    setHeader();
    renderSidebar();
  } catch (e) {
    toast(e.message);
  }
}

// -- wiring ------------------------------------------------------------------------------------

els.composer.addEventListener('submit', (e) => {
  e.preventDefault();
  if (state.activeTurn) stopActive();
  else sendMessage();
});
els.input.addEventListener('input', () => { autosize(); updateComposer(); });
els.input.addEventListener('keydown', (e) => {
  if (e.key === 'Enter' && !e.shiftKey && !e.isComposing) {
    e.preventDefault();
    if (!state.activeTurn) sendMessage();
  }
});
els.thread.addEventListener('click', (e) => {
  const button = e.target.closest('[data-copy-code]');
  if (button) copyText(button.closest('.code').querySelector('code').textContent, button);
});
els.title.addEventListener('click', startRename);
els.titleInput.addEventListener('keydown', (e) => {
  if (e.key === 'Enter') finishRename(true);
  if (e.key === 'Escape') finishRename(false);
});
els.titleInput.addEventListener('blur', () => finishRename(true));
els.deleteChat.addEventListener('click', () => state.current && deleteConversation(state.current));
els.filesBtn.addEventListener('click', openFiles);
els.scheduleBtn.addEventListener('click', () => state.current && cedarSchedule.openCreate(state.current, { model: els.model.value, reasoning: els.reasoning.value }));
$('files-refresh').addEventListener('click', loadFiles);
$('files-close').addEventListener('click', () => els.filesDialog.close());
$('new-chat').addEventListener('click', () => { toggleSidebar(false); location.hash = '#/'; if (!state.current) els.input.focus(); });
$('open-sidebar').addEventListener('click', () => toggleSidebar(true));
$('close-sidebar').addEventListener('click', () => toggleSidebar(false));
els.backdrop.addEventListener('click', () => toggleSidebar(false));
window.addEventListener('hashchange', route);
document.addEventListener('keydown', (e) => {
  if ((e.metaKey || e.ctrlKey) && e.shiftKey && e.key.toLowerCase() === 'o') {
    e.preventDefault();
    location.hash = '#/';
  }
});

async function init() {
  cedarImages.init({ fetchFile: fetchWorkspaceFile, open: cedarViewer.open, download: downloadFile });
  cedarViewer.init({ fetchFile: fetchWorkspaceFile, renderMarkdown, download: downloadFile });
  cedarSchedule.init({ api, toast, renderMarkdown });
  cedarStar.init({ api, toast, onOpen: () => toggleSidebar(false) });
  cedarSettings.init({
    api, toast, authHeaders,
    config: () => state.config,
    current: () => state.current,
    anchor: els.reasoning,
  });
  // The paperclip goes after the Tools button that settings.js puts after the model picker.
  cedarAttach.init({
    form: els.composer,
    textarea: els.input,
    anchor: els.reasoning,
    before: els.composer.querySelector('.composer-row'),
    toast,
  });
  try {
    const res = await fetch('/api/config');
    state.config = await res.json();
  } catch {
    toast('Could not reach the server');
  }
  sendMenu = cedarModels.sendMenu({ send: els.send, modelSelect: els.model, levelSelect: els.reasoning });
  cedarNotify.init({ api, authHeaders, toast, button: els.inboxBtn, current: () => state.current, onChange: loadConversations });
  cedarPairing.init({ api, toast });
  if (state.config.auth_required && !memoryToken) await askToken(false);
  await loadModels();
  cedarSettings.start();
  cedarSchedule.start();
  cedarStar.start();
  await loadConversations();
  route();
  cedarNotify.start();
}

init();

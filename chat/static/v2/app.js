'use strict';

// cedar-chat v2 ("Cedar style"). Plain JavaScript, no build step. Talks only to /api on this
// origin. The markdown renderer and the API/token helpers are the same as in /v1/app.js.
//
// The idea: say on the left, show on the right. The conversation column holds your prompts,
// Cedar's prose, its questions, and one step row per tool call. Everything a tool produced goes
// on the stage beside it as a full-width card, grouped by turn: the document Cedar wrote, a map,
// recipe, trip or stock card, the images it made, and (folded to one line) its searches, commands
// and the pages and files it read. A step row jumps to its card. Cards are built when a tool is
// called and filled in when its result arrives, so nothing is redrawn and widgets keep their state.
//
// From cedar's newer events (trust-view.js draws them; an older cedar sends none and none show):
// the plan it proposes, which it waits on until "start" (POST /api/turns/{id}/plan); approval
// cards for risky calls (POST /api/turns/{id}/approvals/{approval_id}); its reasoning, folded;
// [n] citations linked to a Sources card on the stage; and the turn's cost and sandbox.

const $ = (id) => document.getElementById(id);
const els = {
  app: $('app'), title: $('title'), titleInput: $('title-input'), when: $('when'), status: $('status'), statusText: $('status-text'),
  stop: $('stop'), newChat: $('new-chat'), deleteChat: $('delete-chat'),
  filesBtn: $('files-btn'), filesDialog: $('files-dialog'), filesList: $('files-list'),
  scheduleBtn: $('schedule-btn'), scheduleBadge: $('schedule-badge'), inboxBtn: $('inbox-btn'),
  chatsBtn: $('chats-btn'), drawer: $('drawer'), drawerClose: $('drawer-close'), backdrop: $('backdrop'), recents: $('recents'),
  tabs: $('tabs'), stageBadge: $('stage-badge'),
  talk: $('talk'), notes: $('notes'), splitter: $('splitter'),
  stage: $('stage'), stageBody: $('stage-body'), stageEmpty: $('stage-empty'),
  composer: $('composer'), input: $('input'), send: $('send'), model: $('model'), reasoning: $('reasoning'),
  toast: $('toast'), tokenDialog: $('token-dialog'), tokenForm: $('token-form'),
  tokenInput: $('token-input'), tokenMessage: $('token-message'),
};

const TERMINAL = new Set(['run.completed', 'run.failed', 'run.cancelled']);
const ACTIVE = new Set(['queued', 'running']);
const TOKEN_KEY = 'cedar-chat-token';
const SPLIT_KEY = 'cedar-chat-v2-talk-width';
const DOC_RE = /\.(md|markdown|txt)$/i;
const NO_MODELS = tr('v2.noModels');
const narrow = matchMedia('(max-width: 800px)');

const state = {
  config: { auth_required: false },
  noModels: false,     // cedar has no model registered: messages can't be sent yet
  conversations: [],
  current: null,
  views: new Map(),   // turn id -> view
  order: [],          // turn ids, oldest first
  activeTurn: null,
  sending: false,
  loadToken: 0,
  unseen: 0,          // cards that landed on the stage while a narrow screen showed the conversation
};

// -- small helpers -----------------------------------------------------------------------------

function escapeHtml(s) {
  return String(s).replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c]);
}

function h(tag, className, html) {
  const el = document.createElement(tag);
  if (className) el.className = className;
  if (html !== undefined) el.innerHTML = html;
  return el;
}

/** An element whose text is set safely (tool output is untrusted). */
function t(tag, className, text) {
  const el = h(tag, className);
  el.textContent = text ?? '';
  return el;
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

let toastTimer;
function toast(message) {
  els.toast.textContent = message;
  // The toast is a popover so it can sit in the top layer: re-showing it puts it
  // above any modal dialog (settings, files) opened since, instead of behind it.
  if (els.toast.showPopover) {
    if (els.toast.matches(':popover-open')) els.toast.hidePopover();
    els.toast.showPopover();
    void els.toast.offsetWidth; // start the fade-in from the hidden state
  }
  els.toast.classList.add('show');
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => els.toast.classList.remove('show'), 4000);
}

function storage(op, key, value) {
  try {
    if (op === 'get') return localStorage.getItem(key);
    if (op === 'set') localStorage.setItem(key, value);
    if (op === 'del') localStorage.removeItem(key);
  } catch { /* storage unavailable: the value then lasts for this page only */ }
  return null;
}

let memoryToken = storage('get', TOKEN_KEY);

async function copyText(text, button) {
  try {
    await navigator.clipboard.writeText(text);
    if (button) {
      const old = button.textContent;
      button.textContent = tr('v2.copied');
      setTimeout(() => { button.textContent = old; }, 1500);
    }
  } catch {
    toast(tr('v2.copyFailed'));
  }
}

const fmtMs = (ms) => (typeof ms !== 'number' ? '' : ms < 1000 ? `${cedarI18n.number(ms)} ms`
  : `${cedarI18n.number(ms / 1000, { minimumFractionDigits: 1, maximumFractionDigits: 1 })} s`);
const fmtTime = (iso) => new Date(iso || Date.now()).toLocaleTimeString(cedarI18n.locale, { hour: '2-digit', minute: '2-digit' });

/** Whether a scrolling element is at (or within `slack` of) its end: the reader is following along. */
const nearEnd = (el, slack = 160) => el.scrollHeight - el.scrollTop - el.clientHeight < slack;

// -- markdown (same renderer as /v1/app.js) ----------------------------------------------------

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
    `<button type="button" class="copy-btn" data-copy-code>${escapeHtml(tr('v2.copy'))}</button></div>` +
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
    if (m && ind === base) items.push({ text: [m[3]], rest: [] });
    else if (ind > base && items.length) {
      const item = items[items.length - 1];
      if (!item.rest.length && !RE.item.test(line) && !RE.fence.test(line)) item.text.push(line.trim());
      else item.rest.push(line.slice(Math.min(ind, base + 2)));
    } else if (!m && ind <= base && items.length && !isBlockStart(lines, i)) items[items.length - 1].text.push(line.trim());
    else break;
    i++;
  }
  const body = items.map((it) => `<li>${renderInline(it.text.join(' '))}${it.rest.length ? renderMarkdown(it.rest.join('\n')) : ''}</li>`).join('');
  return [ordered ? `<ol${startNum !== 1 ? ` start="${startNum}"` : ''}>${body}</ol>` : `<ul>${body}</ul>`, i];
}
function renderTable(lines, start) {
  const head = splitRow(lines[start]);
  const align = splitRow(lines[start + 1]).map((c) => c.startsWith(':') && c.endsWith(':') ? 'center' : c.endsWith(':') ? 'right' : '');
  const cell = (tag, text, k) => `<${tag}${align[k] ? ` style="text-align:${align[k]}"` : ''}>${renderInline(text)}</${tag}>`;
  let i = start + 2;
  const rows = [];
  while (i < lines.length && lines[i].trim() && lines[i].includes('|')) {
    const cells = splitRow(lines[i++]);
    rows.push(`<tr>${head.map((_, k) => cell('td', cells[k] ?? '', k)).join('')}</tr>`);
  }
  return [`<div class="table-wrap"><table><thead><tr>${head.map((t, k) => cell('th', t, k)).join('')}</tr></thead><tbody>${rows.join('')}</tbody></table></div>`, i];
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
    } else if ((m = line.match(RE.heading))) { out.push(`<h${m[1].length}>${renderInline(m[2])}</h${m[1].length}>`); i++; }
    else if (RE.hr.test(line)) { out.push('<hr>'); i++; }
    else if (RE.quote.test(line)) {
      const quoted = [];
      while (i < lines.length && RE.quote.test(lines[i])) quoted.push(lines[i++].replace(/^\s*> ?/, ''));
      out.push(`<blockquote>${renderMarkdown(quoted.join('\n'))}</blockquote>`);
    } else if (isTableStart(lines, i)) { const [html, next] = renderTable(lines, i); out.push(html); i = next; }
    else if (RE.item.test(line)) { const [html, next] = parseList(lines, i); out.push(html); i = next; }
    else {
      const para = [lines[i++].trim()];
      while (i < lines.length && lines[i].trim() && !isBlockStart(lines, i)) para.push(lines[i++].trim());
      out.push(`<p>${para.map(renderInline).join('<br>')}</p>`);
    }
  }
  return out.join('');
}

// -- API ---------------------------------------------------------------------------------------

let tokenPrompt = null;
function askToken(rejected) {
  if (!tokenPrompt) {
    tokenPrompt = new Promise((resolve) => {
      els.tokenMessage.textContent = rejected
        ? tr('v2.token.rejected')
        : tr('v2.token.required');
      els.tokenInput.value = '';
      els.tokenForm.onsubmit = () => {
        memoryToken = els.tokenInput.value.trim();
        storage('set', TOKEN_KEY, memoryToken);
        tokenPrompt = null;
        resolve();
      };
      els.tokenDialog.addEventListener('cancel', (e) => e.preventDefault(), { once: true });
      els.tokenDialog.showModal();
    });
  }
  return tokenPrompt;
}
const authHeaders = () => (memoryToken ? { Authorization: `Bearer ${memoryToken}` } : {});
async function api(method, path, body) {
  const headers = authHeaders();
  if (body !== undefined) headers['Content-Type'] = 'application/json';
  const res = await fetch(`/api${path}`, { method, headers, body: body === undefined ? undefined : JSON.stringify(body) });
  if (res.status === 401) { await askToken(true); return api(method, path, body); }
  if (res.status === 204) return null;
  const data = await res.json().catch(() => null);
  if (!res.ok) throw new Error(data?.error?.message || tr('v2.requestFailed', { status: String(res.status) }));
  return data;
}

// -- what each tool is -------------------------------------------------------------------------

function workspacePath(path) {
  if (typeof path !== 'string') return null;
  let p = path.trim();
  if (p.startsWith('/workspace/')) p = p.slice('/workspace/'.length);
  else if (p.startsWith('./')) p = p.slice(2);
  if (!p || p.startsWith('/') || p.split('/').some((part) => !part || part === '.' || part === '..')) return null;
  return p;
}

const firstLine = (s) => String(s || '').split('\n').find((l) => l.trim()) || '';

// How each tool reads as a step ("Ran ls" / "Running ls") and in a card's title, and what its
// one-line subject is. `verb` names the texts v2.verb.<verb>.doing / .did, templates with a
// {subject}. `code` marks a subject that is code. Tools not listed fall back to their name.
const TOOLS = {
  web_search: { verb: 'search', subject: (i) => tr('v2.quoted', { text: i.query || '' }), icon: 'search' },
  bash: { verb: 'run', subject: (i) => `$ ${firstLine(i.command)}`, code: true, icon: 'terminal' },
  run_python: { verb: 'run', subject: (i) => firstLine(i.code), code: true, icon: 'python' },
  read_file: { verb: 'read', subject: (i) => i.path, icon: 'file' },
  read_document: { verb: 'read', subject: (i) => i.path, icon: 'file' },
  fetch_url: { verb: 'fetch', subject: (i) => i.url, icon: 'globe' },
  download_file: { verb: 'download', subject: (i) => (i.path ? `${i.url} → ${i.path}` : i.url), icon: 'download' },
  write_file: { verb: 'write', subject: (i) => i.path, icon: 'doc' },
  generate_pdf: { verb: 'write', subject: (i) => i.path, icon: 'doc' },
  generate_chart: { verb: 'draw', subject: (i) => i.path, icon: 'chart' },
  weather_forecast: {
    verb: 'weather',
    subject: (i) => i.location || (i.latitude != null ? `${i.latitude}, ${i.longitude}` : ''),
    icon: 'weather',
  },
  show_map: { verb: 'map', subject: (i) => mapSubject(i), icon: 'map' },
  show_recipe: { verb: 'recipe', subject: (i) => i.title, icon: 'recipe' },
  plan_trip: { verb: 'trip', subject: (i) => tripSubject(i), icon: 'trip' },
  compare_stocks: { verb: 'stocks', subject: (i) => stocksSubject(i), icon: 'stocks' },
  search_products: { verb: 'shop', subject: (i) => (typeof i.query === 'string' ? i.query : ''), icon: 'shop' },
  search_flights: { verb: 'flights', subject: (i) => flightsSubject(i), icon: 'plane' },
  ask_user: { verb: 'ask', subject: (i) => questionsSubject(i), icon: 'question' },
  remember: { verb: 'remember', subject: (i) => (typeof i.text === 'string' ? i.text : ''), icon: 'bookmark' },
  forget: { verb: 'forget', subject: () => '', icon: 'bookmark' },
};

/** The step text of `verb` ("Searched {subject}") with `subject` put in for {subject}: as HTML
 *  when `html` is set (`subject` is HTML then), else as plain text. An empty subject leaves no
 *  dangling space or colon. */
function verbText(verb, done, subject, html) {
  const esc = html ? escapeHtml : String;
  const [before, after = ''] = tr(`v2.verb.${verb}.${done ? 'did' : 'doing'}`).split('{subject}');
  const text = `${esc(before)}${subject}${esc(after)}`;
  return subject ? text.trim() : text.replace(/[\s:\u00a0]+$/, '').trim();
}

const ICONS = {
  search: '<circle cx="11" cy="11" r="6"/><path d="M20 20l-4.5-4.5"/>',
  terminal: '<path d="M5 7l5 5-5 5M12 17h7"/>',
  python: '<path d="M8 4h8v5H8zM8 15h8v5H8zM4 9h16v6H4z"/>',
  file: '<path d="M7 3h7l5 5v13H7z"/><path d="M14 3v5h5M10 13h5M10 17h5"/>',
  globe: '<circle cx="12" cy="12" r="9"/><path d="M3 12h18M12 3a14 14 0 0 1 0 18M12 3a14 14 0 0 0 0 18"/>',
  download: '<path d="M12 4v11M7 10l5 5 5-5M5 20h14"/>',
  doc: '<path d="M6 3h9l4 4v14H6z"/><path d="M9 11h7M9 15h7M9 7h3"/>',
  chart: '<path d="M4 20h16M7 16V9M12 16V5M17 16v-6"/>',
  weather: '<path d="M7 17a4 4 0 1 1 1-7.9A5.5 5.5 0 0 1 18.5 11 3 3 0 0 1 18 17z"/>',
  map: '<path d="M3 6l6-2 6 2 6-2v14l-6 2-6-2-6 2z"/><path d="M9 4v14M15 6v14"/>',
  recipe: '<path d="M6 3v7a3 3 0 0 0 6 0V3M9 3v18M18 3c-2 1-3 3-3 7h3v11"/>',
  trip: '<circle cx="6" cy="18" r="2"/><circle cx="18" cy="6" r="2"/><path d="M8 17h5a3 3 0 0 0 0-6h-2a3 3 0 0 1 0-6h5"/>',
  stocks: '<path d="M3 17l5-5 4 3 5-7 4 4"/><path d="M17 8h4v4"/>',
  shop: '<path d="M5 8h14l-1 12H6zM9 8V6a3 3 0 0 1 6 0v2"/>',
  plane: '<path d="M12 2c.8 0 1.5.7 1.5 1.5V9l7.5 4.5V16l-7.5-2.5V18l2 1.5V21L12 20l-3.5 1v-1.5l2-1.5v-4.5L3 16v-2.5L10.5 9V3.5C10.5 2.7 11.2 2 12 2z"/>',
  bookmark: '<path d="M6 3h12v18l-6-4-6 4z"/>',
  question: '<circle cx="12" cy="12" r="9"/><path d="M9.5 9.5a2.5 2.5 0 1 1 3.5 2.3c-.7.4-1 1-1 1.7M12 17h.01"/>',
  image: '<rect x="3" y="5" width="18" height="14" rx="2"/><circle cx="9" cy="10" r="1.5"/><path d="M21 16l-5-5-9 8"/>',
  tool: '<path d="M14 4a4 4 0 0 0 5 5l-9 9a2.5 2.5 0 0 1-3.5-3.5l9-9z"/><path d="M5 19l1-1"/>',
};
const icon = (name) => `<svg viewBox="0 0 24 24" aria-hidden="true">${ICONS[name] || ICONS.tool}</svg>`;

/** What a search_flights call is about: "SFO → MUC", several airports joined with "/". */
function flightsSubject(input) {
  const side = (v) => (Array.isArray(v) ? v : typeof v === 'string' ? v.split(',') : [])
    .map((c) => (typeof c === 'string' ? c.trim() : '')).filter(Boolean).join('/');
  return `${side(input.from) || '?'} → ${side(input.to) || '?'}`;
}

/** What a compare_stocks call is about: its symbols. */
function stocksSubject(input) {
  const symbols = Array.isArray(input.symbols) ? input.symbols : [input.symbols];
  return symbols.filter((s) => typeof s === 'string').join(' · ');
}

/** What an ask_user call is about: each question's header, else the question itself. */
function questionsSubject(input) {
  const questions = Array.isArray(input.questions) ? input.questions : [];
  return questions.map((q) => q?.header || q?.question).filter(Boolean).join(' · ');
}

/** What a plan_trip call is about: its title, else "from → to". */
function tripSubject(input) {
  if (input.title) return input.title;
  const end = (e) => (typeof e === 'string' ? e : e?.label || e?.location || (e ? `${e.latitude}, ${e.longitude}` : '?'));
  return `${end(input.from)} → ${end(input.to)}`;
}

/** What a show_map call is about: its title, else the places it names. */
function mapSubject(input) {
  if (input.title) return input.title;
  const places = Array.isArray(input.markers) ? input.markers : [];
  return places.map((m) => m?.location || m?.label || `${m?.latitude}, ${m?.longitude}`).join(' · ');
}

// Tools whose `path` input is a file they write, so a finished call gets View and Download buttons.
const FILE_TOOLS = new Set(['write_file', 'generate_pdf', 'generate_chart', 'download_file']);
// Tools whose result is drawn as a widget (map-view.js and friends), which keeps state once built.
const WIDGETS = {
  show_map: { lib: () => cedarMap, waiting: 'v2.wait.map', count: (s) => tr('v2.count.places', { count: s.markers.length }) },
  show_recipe: { lib: () => cedarRecipe, waiting: 'v2.wait.recipe', count: (s) => tr('v2.count.steps', { count: s.steps.length }) },
  plan_trip: { lib: () => cedarTransit, waiting: 'v2.wait.trip', count: (s) => tr('v2.count.ways', { count: s.options.length }) },
  compare_stocks: { lib: () => cedarStocks, waiting: 'v2.wait.stocks', count: (s) => tr('v2.count.stocks', { count: s.stocks.length }) },
  search_products: { lib: () => cedarProducts, waiting: 'v2.wait.shop', count: (s) => tr('v2.count.options', { count: s.options.length }) },
  search_flights: {
    lib: () => cedarFlights,
    waiting: 'v2.wait.flights',
    count: (s) => (s.kind === 'booking' ? tr('v2.count.sellers', { count: s.sellers.length }) : tr('v2.count.flights', { count: s.options.length })),
  },
  // Drawn from the call's data rather than its output, which only says where the picture went.
  generate_chart: { lib: () => cedarChart, waiting: 'v2.wait.chart', count: () => tr('v2.saved') },
};

const subjectOf = (tool) => (TOOLS[tool.name]?.subject(tool.input || {}) || '').trim();

/** "Ran <code>$ ls</code>", "Searching “x”", or the bare tool name: HTML for a step or card title. */
function toolHtml(tool, done) {
  const spec = TOOLS[tool.name];
  if (!spec) return done ? escapeHtml(tool.name) : escapeHtml(tr('v2.runningTool', { name: tool.name }));
  const subject = subjectOf(tool);
  const subjectHtml = spec.code ? `<code>${escapeHtml(subject)}</code>` : escapeHtml(subject);
  return verbText(spec.verb, done, subjectHtml, true);
}

/** The same, as plain text, for the header's status and tooltips. */
function toolText(tool, done) {
  const spec = TOOLS[tool.name];
  if (!spec) return done ? tool.name : tr('v2.runningTool', { name: tool.name });
  return verbText(spec.verb, done, subjectOf(tool), false);
}

/** A tool call that produces a document: a Markdown/text file, or a PDF made from Markdown. */
function isDocument(tool) {
  const { path, format } = tool.input || {};
  if (typeof path !== 'string') return false;
  if (tool.name === 'write_file') return DOC_RE.test(path);
  return tool.name === 'generate_pdf' && (!format || format === 'markdown');
}

/** Brave results as cedar formats them: "1. Title\n   url (age)\n   snippet". */
function parseSearchResults(output) {
  const results = [];
  for (const block of String(output || '').split(/\n\s*\n/)) {
    const lines = block.split('\n').map((l) => l.trim()).filter(Boolean);
    const m = lines[0]?.match(/^\d+\.\s+(.*)$/);
    if (!m || !lines[1]) continue;
    const u = lines[1].match(/^(\S+)(?:\s+\((.+)\))?$/);
    results.push({ title: m[1], url: u?.[1] || '', age: u?.[2] || '', snippet: lines.slice(2).join(' ') });
  }
  return results;
}

function hostOf(url) { try { return new URL(url).hostname.replace(/^www\./, ''); } catch { return url; } }

/** A link for http(s) URLs only; anything else (tool output is untrusted) stays plain text. */
function linkHtml(url, label) {
  const text = escapeHtml(label ?? url);
  return /^https?:\/\//i.test(url) ? `<a href="${escapeHtml(url)}" target="_blank" rel="noopener noreferrer">${text}</a>` : text;
}

// -- the stage: one card per tool call -----------------------------------------------------------

function downloadLink(path, primary) {
  const b = h('button', `copy-btn${primary ? ' primary' : ''}`, escapeHtml(tr('v2.download')));
  b.type = 'button';
  b.title = tr('v2.downloadNamed', { path });
  b.addEventListener('click', () => downloadFile(path, b));
  return b;
}

/** Rendered Markdown with the workspace images it refers to. */
function markdownEl(text, onLoad) {
  const md = h('div', 'md', renderMarkdown(text));
  cedarImages.hydrate(md, onLoad ? { onLoad } : undefined);
  return md;
}

/** A labelled block of output (a command's result, a fetched page, …). */
function outputBlock(text, label, extraClass) {
  const frag = document.createDocumentFragment();
  if (label) frag.append(t('div', 'out-label', label));
  frag.append(t('pre', `out${extraClass ? ` ${extraClass}` : ''}`, text));
  return frag;
}

/** The kind of card a tool call gets: doc, widget, file, trace, or none (its result lives elsewhere). */
function cardKind(tool) {
  if (tool.name === 'ask_user') return tool.is_error ? 'trace' : 'none'; // the questions are a form in the conversation
  if (isDocument(tool) && tool.is_error !== true) return 'doc';
  if (WIDGETS[tool.name] && tool.is_error !== true) return 'widget';
  if (FILE_TOOLS.has(tool.name) && tool.is_error !== true) return 'file';
  // A tool that printed charts (`[chart] {spec}` lines, see chart-view.js) gets them drawn.
  if (tool.output !== undefined && !tool.is_error && cedarChart.fromOutput(tool.output).charts.length) return 'charts';
  return 'trace';
}

/** The stage group for a turn, made when its first card lands. */
function ensureGroup(view) {
  if (view.group) return view.group;
  const group = h('section', 'turn');
  group.dataset.turn = view.turn.id;
  const n = state.order.indexOf(view.turn.id) + 1;
  const head = h('div', 'turn-head');
  head.append(t('span', 'n', tr('v2.turn', { n })), t('span', 'q', firstLine(view.turn.prompt).slice(0, 160)), t('span', 't', fmtTime(view.turn.created_at)));
  head.querySelector('.q').title = view.turn.prompt;
  group.append(head);
  // Turns are replayed side by side when a conversation opens, so a group goes after the groups of earlier turns.
  const later = state.order.slice(state.order.indexOf(view.turn.id) + 1).map((id) => state.views.get(id)?.group).find((g) => g?.isConnected);
  if (later) els.stageBody.insertBefore(group, later); else els.stageBody.append(group);
  view.group = group;
  els.stageEmpty.hidden = true;
  return group;
}

/** Put a card on the stage for `view`, following along if the reader was at the end. */
function placeCard(view, card) {
  const follow = nearEnd(els.stage, 240) || view === state.activeTurn;
  ensureGroup(view).append(card);
  if (follow) els.stage.scrollTop = els.stage.scrollHeight;
  if (narrow.matches && els.app.dataset.pane !== 'stage') { state.unseen++; updateTabs(); }
}

/** After a picture or map on the stage loads: stay at the end if the reader was there. */
function keepStageAtEnd() {
  if (nearEnd(els.stage, 240)) els.stage.scrollTop = els.stage.scrollHeight;
}

function cardHead(iconName, titleHtml, { meta, actions, chevron } = {}) {
  const head = h('div', 'card-head');
  head.append(h('span', 'ico', icon(iconName)));
  if (chevron) head.insertAdjacentHTML('beforeend', '<svg class="chev" viewBox="0 0 24 24" aria-hidden="true"><path d="M9 6l6 6-6 6"/></svg>');
  const ttl = h('span', 'ttl', titleHtml);
  head.append(ttl);
  const m = h('span', 'meta');
  if (meta) m.append(...(Array.isArray(meta) ? meta : [meta]));
  head.append(m);
  if (actions?.length) { const a = h('span', 'acts'); a.append(...actions); head.append(a); }
  return head;
}

const spinnerMeta = (text) => { const s = document.createDocumentFragment(); s.append(h('span', 'spinner'), t('span', '', text)); return s; };

/** The card for a tool call (`tool.card` once made); `fillCard` draws its current state. */
function cardFor(tool) {
  if (tool.card) return tool.card;
  const card = h('article', 'card');
  card.dataset.tool = tool.id;
  tool.card = card;
  fillCard(tool);
  return card;
}

function fillCard(tool) {
  const card = tool.card;
  const kind = cardKind(tool);
  const done = tool.output !== undefined;
  const err = done && tool.is_error;
  card.className = `card ${kind}${!done ? ' running' : ''}${err ? ' err' : ''}${tool.open ? ' open' : ''}`;
  card.textContent = '';
  if (kind === 'none') { card.hidden = true; return; }
  card.hidden = false;
  if (kind === 'doc') return fillDocument(tool, card, done);
  if (kind === 'widget') return fillWidget(tool, card, done);
  if (kind === 'file') return fillFile(tool, card, done);
  if (kind === 'charts') return fillCharts(tool, card);
  return fillTrace(tool, card, done);
}

/** A tool that printed charts: the charts, and the rest of its output folded under them. */
function fillCharts(tool, card) {
  const { charts, text } = cedarChart.fromOutput(tool.output);
  card.classList.add('widget'); // laid out like the other widget cards
  tool.chartEls ??= charts.map((spec) => cedarChart.element(spec));
  tool.reportEl ??= cedarChart.report(text);
  const meta = t('span', '', `${tr('v2.count.charts', { count: charts.length })} · ${fmtMs(tool.duration_ms)}`);
  card.append(cardHead(TOOLS[tool.name]?.icon || 'chart', toolHtml(tool, true), { meta }));
  const body = h('div', 'card-body');
  body.append(...tool.chartEls, tool.reportEl);
  card.append(body);
}

function fillDocument(tool, card, done) {
  const path = String(tool.input.path);
  const name = path.split('/').pop();
  const content = String(tool.input.content ?? '');
  const size = tool.name === 'generate_pdf' ? 'PDF' : tr('v2.size.b', { n: new Blob([content]).size });
  card.classList.toggle('drafting', !done);
  const copy = h('button', 'copy-btn', escapeHtml(tr('v2.copy')));
  copy.type = 'button';
  copy.addEventListener('click', () => copyText(content, copy));
  const actions = [copy];
  const rel = done && workspacePath(path);
  if (rel) {
    // The card shows a PDF's Markdown source; the finished PDF opens in the viewer.
    const view = tool.name === 'generate_pdf' ? cedarViewer.viewButton(rel, { label: tr('v2.viewPdf') }) : null;
    if (view) actions.push(view);
    actions.push(downloadLink(rel, true));
  }
  const meta = done ? t('span', '', `${fmtMs(tool.duration_ms)} · ${size}`) : spinnerMeta(tr('v2.wait.doc'));
  card.append(cardHead('doc', escapeHtml(name), { meta, actions }));
  const body = h('div', 'card-body');
  if (/\.txt$/i.test(path)) body.append(t('pre', 'plain', content));
  else body.append(markdownEl(content, keepStageAtEnd));
  card.append(body);
}

function fillWidget(tool, card, done) {
  const w = WIDGETS[tool.name];
  const spec = done ? w.lib().parse(tool.output, tool.input) : null;
  const iconName = TOOLS[tool.name].icon;
  if (!done) { card.append(cardHead(iconName, toolHtml(tool, false), { meta: spinnerMeta(tr(w.waiting)) })); return; }
  // A file the widget can't draw is still a saved file.
  if (!spec && FILE_TOOLS.has(tool.name)) { fillFile(tool, card, done); return; }
  if (!spec) {
    // Not a usable result: fall back to the raw output, folded like a trace.
    card.classList.add('trace');
    card.append(cardHead(iconName, toolHtml(tool, true), { meta: t('span', '', fmtMs(tool.duration_ms)), chevron: true }));
    const body = h('div', 'card-body');
    body.append(outputBlock(String(tool.output)));
    card.append(body);
    bindTrace(tool, card);
    return;
  }
  const key = `${tool.name}El`;
  tool[key] ??= w.lib().element(spec);
  // A widget that saved a file (a chart) keeps the file's View and Download buttons.
  const actions = [];
  const rel = FILE_TOOLS.has(tool.name) && workspacePath(String(tool.input?.path || ''));
  if (rel) {
    const view = cedarViewer.viewButton(rel);
    if (view) actions.push(view);
    actions.push(downloadLink(rel));
  }
  card.append(cardHead(iconName, toolHtml(tool, true), { meta: t('span', '', `${w.count(spec)} · ${fmtMs(tool.duration_ms)}`), actions }));
  const body = h('div', 'card-body');
  body.append(tool[key]);
  card.append(body);
}

function fillFile(tool, card, done) {
  const path = String(tool.input?.path || '');
  const actions = [];
  const rel = done && workspacePath(path);
  if (rel) {
    const view = cedarViewer.viewButton(rel);
    if (view) actions.push(view);
    actions.push(downloadLink(rel));
  }
  const meta = done ? t('span', '', `${tr('v2.saved')} · ${fmtMs(tool.duration_ms)}`) : spinnerMeta(tr('v2.wait.file'));
  card.append(cardHead(TOOLS[tool.name].icon, toolHtml(tool, done), { meta, actions }));
}

function fillTrace(tool, card, done) {
  card.classList.add('trace');
  const input = tool.input || {};
  const spec = TOOLS[tool.name];
  const iconName = spec?.icon || 'tool';
  const err = done && tool.is_error;
  const body = h('div', 'card-body');
  let meta;
  if (!done) meta = spinnerMeta(tr('v2.running'));
  else if (err) meta = t('span', '', tr('v2.failedShort'));
  else meta = t('span', '', fmtMs(tool.duration_ms));
  let title = toolHtml(tool, done);

  switch (tool.name) {
    case 'web_search': {
      const results = done && !err ? parseSearchResults(tool.output) : [];
      if (results.length) {
        meta = t('span', '', `${tr('v2.count.results', { count: results.length })} · ${fmtMs(tool.duration_ms)}`);
        const list = h('div', 'results');
        for (const r of results) {
          const item = h('div', 'result');
          item.append(h('div', 'head', linkHtml(r.url, r.title)));
          if (r.url) item.append(t('div', 'host', [hostOf(r.url), r.age].filter(Boolean).join(' · ')));
          if (r.snippet) item.append(t('div', 'snip', r.snippet));
          list.append(item);
        }
        body.append(list);
      } else if (done) body.append(outputBlock(String(tool.output).trim() || tr('v2.noResults')));
      break;
    }
    case 'bash': {
      const command = String(input.command || '');
      if (command.includes('\n')) body.append(outputBlock(command, tr('v2.label.command'), 'cmd'));
      if (done) body.append(outputBlock(String(tool.output).trim() || tr('v2.noOutput'), command.includes('\n') ? tr('v2.label.output') : ''));
      break;
    }
    case 'run_python': {
      body.append(outputBlock(String(input.code || ''), tr('v2.label.code'), 'cmd'));
      if (done) body.append(outputBlock(String(tool.output).trim() || tr('v2.noOutput'), tr('v2.label.output')));
      break;
    }
    case 'fetch_url':
      title = verbText('fetch', done, linkHtml(String(input.url || '')), true);
      if (done) body.append(outputBlock(String(tool.output)));
      break;
    case 'read_file':
    case 'read_document':
    case 'weather_forecast':
    case 'ask_user':
      if (done) body.append(outputBlock(String(tool.output)));
      break;
    default:
      if (FILE_TOOLS.has(tool.name) || WIDGETS[tool.name]) {
        // A file or widget tool that failed: what it was asked and what came back.
        if (done) body.append(outputBlock(String(tool.output)));
      } else {
        // A tool this page doesn't know yet: still show what it was given and what it returned.
        if (!spec) title = done ? `<code>${escapeHtml(tool.name)}</code>` : verbText('run', false, `<code>${escapeHtml(tool.name)}</code>`, true);
        body.append(outputBlock(JSON.stringify(input, null, 2), tr('v2.label.input'), 'cmd'));
        if (done) body.append(outputBlock(String(tool.output).trim() || tr('v2.noOutput'), tr('v2.label.output')));
      }
  }
  card.append(cardHead(iconName, title, { meta, chevron: true }));
  if (body.childNodes.length) card.append(body);
  bindTrace(tool, card);
}

/** A trace card opens and closes from its head (a link inside it still works as a link). */
function bindTrace(tool, card) {
  const head = card.querySelector('.card-head');
  head.setAttribute('role', 'button');
  head.setAttribute('aria-expanded', String(Boolean(tool.open)));
  head.tabIndex = 0;
  const toggle = () => { tool.open = !tool.open; card.classList.toggle('open', tool.open); head.setAttribute('aria-expanded', String(tool.open)); };
  head.addEventListener('click', (e) => { if (!e.target.closest('a, button')) toggle(); });
  head.addEventListener('keydown', (e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); toggle(); } });
}

/** The card of figures a response produced (its charts and pictures), after its other cards. */
function figuresCard(figures) {
  const card = h('article', 'card figures');
  const n = figures.filter((f) => f.classList?.contains('cedar-image')).length;
  card.append(cardHead('image', escapeHtml(tr('v2.count.images', { count: n })), { meta: t('span', '', tr('v2.fromResponse')) }));
  const body = h('div', 'card-body');
  body.append(...figures);
  card.append(body);
  return card;
}

/** Scroll the stage to a tool's card (opening a folded one) and flash it. */
function focusCard(tool) {
  const card = tool.card;
  if (!card || card.hidden) return;
  if (narrow.matches) setPane('stage');
  if (card.classList.contains('trace') && !tool.open) {
    tool.open = true;
    card.classList.add('open');
    card.querySelector('.card-head')?.setAttribute('aria-expanded', 'true');
  }
  card.scrollIntoView({ block: 'center', behavior: narrow.matches ? 'auto' : 'smooth' });
  card.classList.remove('flash');
  requestAnimationFrame(() => card.classList.add('flash'));
}

// -- the conversation column -------------------------------------------------------------------

function note(cls, who, html) {
  const el = h('div', `note ${cls}`);
  if (who) el.append(h('div', 'who', who));
  const body = h('div', cls === 'cedar' ? 'md' : '', html);
  if (cls === 'cedar') cedarImages.hydrate(body, { onLoad: keepNotesAtEnd });
  el.append(body);
  return el;
}

function addTurn(turn) {
  const view = {
    turn, texts: [], textNotes: [], tools: [], images: [], byId: new Map(), usage: { input: 0, output: 0 },
    abort: new AbortController(), done: false,
    started: null,        // run.started, for the sandbox line
    planEl: null,         // the plan, while there is one
    approvals: new Map(), // approval id -> its card
    approvalsGranted: 0,
    reasoningEl: null,    // the fold reasoning is being added to
    workingEl: null,
    stepsEl: null,  // the open run of step rows; a note from Cedar closes it
    group: null,    // this turn's group of cards on the stage
  };
  state.views.set(turn.id, view);
  state.order.push(turn.id);
  const user = note('user', escapeHtml(tr('v2.you', { time: fmtTime(turn.created_at) })));
  user.firstChild.append(cedarTrace.button(turn.id));
  if (turn.attachments?.length) user.lastChild.append(cedarAttach.chips(turn.attachments));
  user.lastChild.append(document.createTextNode(turn.prompt));
  cedarClarify.settle(els.notes); // this message answers any question still open above it
  els.notes.append(user);
  view.workingEl = h('div', 'working');
  view.workingEl.append(h('span', 'spinner'), t('span', '', tr('v2.thinking')));
  els.notes.append(view.workingEl);
  if (ACTIVE.has(turn.status)) state.activeTurn = view;
  updateChrome();
  followEvents(view);
  return view;
}

/** After a picture in the conversation loads: stay at the end if the reader was there. */
function keepNotesAtEnd() {
  if (nearEnd(els.notes)) els.notes.scrollTop = els.notes.scrollHeight;
}

function placeNote(view, el) {
  const follow = nearEnd(els.notes) || view === state.activeTurn;
  els.notes.insertBefore(el, view.workingEl);
  if (follow) els.notes.scrollTop = els.notes.scrollHeight;
}

/** What the turn is doing, for the header; the working line under the notes just says so, since
 *  the step row above it already names the tool. `mode` is 'thinking' or 'waiting' for those
 *  states, which the working line shows as they are. */
function setWorking(view, text, mode) {
  view.doing = text;
  const waiting = mode === 'waiting';
  if (view.workingEl) {
    view.workingEl.lastChild.textContent = mode ? text : tr('v2.working');
    view.workingEl.classList.toggle('waiting', waiting);
  }
  updateChrome();
}

/** A step row for a tool call, in the conversation, that jumps to its card. */
function addStep(view, tool) {
  if (!view.stepsEl) {
    view.stepsEl = h('div', 'steps');
    placeNote(view, view.stepsEl);
  }
  const row = h('button', 'step running');
  row.type = 'button';
  row.append(t('span', 'i', String(view.tools.length)), h('span', 'what'), t('span', 'ms', ''));
  row.addEventListener('click', () => focusCard(tool));
  tool.step = row;
  view.stepsEl.append(row);
  fillStep(tool);
  if (nearEnd(els.notes, 200) || view === state.activeTurn) els.notes.scrollTop = els.notes.scrollHeight;
}

function fillStep(tool) {
  const row = tool.step;
  if (!row) return;
  const done = tool.output !== undefined;
  row.className = `step ${!done ? 'running' : tool.is_error ? 'err' : 'done'}`;
  row.querySelector('.what').innerHTML = toolHtml(tool, done);
  row.querySelector('.ms').textContent = !done ? '…' : tool.is_error ? tr('v2.failedShort') : fmtMs(tool.duration_ms);
  const where = cardKind(tool) === 'none' ? '' : ` · ${tr('v2.step.seeResult')}`;
  row.title = toolText(tool, done) + where;
}

// -- the plan, approvals, reasoning and sources ------------------------------------------------

/** The sources a turn has read so far, with the numbers Cedar cites them by. */
const sourcesOf = (view) => cedarTrust.sources.collect(view.tools, parseSearchResults);

/** Link "[n]" in Cedar's notes and the documents it wrote. Safe to repeat. */
function citeAll(view) {
  const sources = sourcesOf(view);
  if (!sources.length) return;
  for (const n of view.textNotes) cedarTrust.sources.cite(n.lastChild, sources);
  for (const tool of view.tools) {
    if (tool.card && cardKind(tool) === 'doc') cedarTrust.sources.cite(tool.card.querySelector('.card-body'), sources);
  }
}

/** The plan Cedar sent: editable while the run waits on it, a checklist once it is going. */
function showPlan(view, data, awaiting) {
  const plan = cedarTrust.plan.parse(data);
  if (!plan) return;
  view.planData = data;
  // Only a turn still going can be answered (a finished one is replayed when a chat opens).
  const waits = awaiting && !view.done && ACTIVE.has(view.turn.status);
  const el = cedarTrust.plan.element(plan, {
    awaiting: waits,
    note: waits ? cedarTrust.plan.awaitingNote(plan) : data.by === 'user' ? tr('v2.plan.agreed') : data.by === 'timeout' ? tr('v2.plan.timeout') : '',
    onApprove: (steps) => approvePlan(view, steps),
  });
  if (view.planEl) view.planEl.replaceWith(el); else placeNote(view, el);
  view.planEl = el;
  view.stepsEl = null;
  if (waits) setWorking(view, tr('v2.waiting.plan'), 'waiting');
}

async function approvePlan(view, steps) {
  try {
    await api('POST', `/turns/${view.turn.id}/plan`, { steps });
  } catch (e) { toast(e.message); throw e; }
  setWorking(view, tr('v2.thinking'), 'thinking');
}

/** A risky call waits on the reader: the card with Allow once / Always / Skip. */
function showApproval(view, d) {
  const spec = { id: d.id, tool: d.tool, input: d.input || {}, reason: d.reason, scope: d.scope, rule: d.rule };
  const el = cedarTrust.approval.element(spec, {
    onDecide: async (decision) => {
      try { await api('POST', `/turns/${view.turn.id}/approvals/${encodeURIComponent(spec.id)}`, { decision }); }
      catch (e) { toast(e.message); throw e; }
      setWorking(view, tr('v2.thinking'), 'thinking');
    },
  });
  // A replayed turn that is over can't be answered any more.
  if (view.done || !ACTIVE.has(view.turn.status)) el.querySelector('.cedar-approval-actions').hidden = true;
  view.approvals.set(String(d.id), el);
  placeNote(view, el);
  view.stepsEl = null;
  setWorking(view, tr('v2.waiting.approval'), 'waiting');
}

/** Cedar's reasoning, folded; text for the same stretch of thinking is added to one fold. */
function showReasoning(view, d) {
  if (!view.reasoningEl) {
    view.reasoningEl = cedarTrust.reasoning.element(d.text || '', d.duration_ms);
    placeNote(view, view.reasoningEl);
    view.stepsEl = null;
  } else {
    view.reasoningEl.cedarReasoning.append(d.text || '');
    if (typeof d.duration_ms === 'number') view.reasoningEl.cedarReasoning.done(d.duration_ms);
  }
  if (d.done) view.reasoningEl = null;
}

/** The numbered sources of a finished turn, as a card after its other cards. */
function sourcesCard(sources) {
  const card = h('article', 'card sources');
  card.append(cardHead('globe', escapeHtml(tr('v2.sources', { count: sources.length })), { meta: t('span', '', tr('v2.citedAs')) }));
  const body = h('div', 'card-body');
  body.append(cedarTrust.sources.element(sources, { onOpen: (s) => s.path && downloadFile(workspacePath(s.path)) }));
  card.append(body);
  return card;
}

// -- events ------------------------------------------------------------------------------------

function parseSse(raw) {
  let data = '';
  for (const line of raw.split('\n')) if (line.startsWith('data:')) data += line.slice(line[5] === ' ' ? 6 : 5);
  if (!data) return null;
  try { return JSON.parse(data); } catch { return null; }
}

async function followEvents(view) {
  let after = 0;
  let failures = 0;
  while (!view.done) {
    try {
      const res = await fetch(`/api/turns/${view.turn.id}/events?after=${after}`, { headers: authHeaders(), signal: view.abort.signal });
      if (res.status === 401) { await askToken(true); continue; }
      if (!res.ok) {
        const data = await res.json().catch(() => null);
        const err = new Error(data?.error?.message || tr('v2.loadResponseFailed', { status: String(res.status) }));
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
      if (e.fatal || ++failures > 5) { finishTurn(view, { type: 'run.failed', data: { error: e.message } }); return; }
    }
    if (!view.done) await sleep(Math.min(1000 * 2 ** failures, 8000));
  }
}

function applyEvent(view, event) {
  const d = event.data || {};
  switch (event.type) {
    case 'run.started':
      view.started = d;
      return;
    case 'plan':
      showPlan(view, d, d.awaiting === true);
      break;
    case 'plan.updated':
      showPlan(view, d, false);
      setWorking(view, tr('v2.thinking'), 'thinking');
      break;
    case 'assistant.reasoning':
      showReasoning(view, d);
      break;
    case 'approval.required':
      showApproval(view, d);
      break;
    case 'approval.resolved': {
      const el = view.approvals.get(String(d.id));
      if (el && el.dataset.state === 'pending') {
        cedarTrust.approval.settle(el, d.decision, event.created_at);
        el.querySelector('.cedar-approval-actions').hidden = false;
      }
      if (d.decision === 'allow' || d.decision === 'allow_always') view.approvalsGranted++;
      setWorking(view, tr('v2.thinking'), 'thinking');
      break;
    }
    case 'assistant.text': {
      view.texts.push(d.text);
      view.reasoningEl = null;
      const n = note('cedar', 'Cedar', renderMarkdown(d.text));
      view.textNotes.push(n);
      const sources = sourcesOf(view);
      if (sources.length) cedarTrust.sources.cite(n.lastChild, sources);
      placeNote(view, n);
      view.stepsEl = null; // the next tool call starts a new run of steps, after this note
      setWorking(view, tr('v2.thinking'), 'thinking');
      break;
    }
    case 'tool.call': {
      const tool = { id: d.id, name: d.name, input: d.input || {}, open: false, stepId: d.step ?? null };
      view.tools.push(tool);
      view.byId.set(d.id, tool);
      view.reasoningEl = null;
      if (tool.stepId != null) view.planEl?.cedarPlan.mark(tool.stepId, 'running');
      addStep(view, tool);
      const card = cardFor(tool);
      if (!card.hidden) placeCard(view, card);
      setWorking(view, toolText(tool, false));
      break;
    }
    case 'tool.result': {
      const tool = view.byId.get(d.id);
      if (tool) {
        Object.assign(tool, { output: String(d.output ?? ''), is_error: Boolean(d.is_error), duration_ms: d.duration_ms, sources: d.sources });
        if (tool.is_error) tool.open = true; // a failure is worth reading
        fillStep(tool);
        if (tool.stepId != null) view.planEl?.cedarPlan.mark(tool.stepId, tool.is_error ? 'error' : 'done');
        const wasHidden = tool.card.hidden;
        const follow = nearEnd(els.stage, 240) || view === state.activeTurn;
        fillCard(tool);
        if (wasHidden && !tool.card.hidden) placeCard(view, tool.card);
        else if (follow && !tool.card.hidden) els.stage.scrollTop = els.stage.scrollHeight;
        // Questions for the user are a form in the conversation, closed if a later message answered them.
        const spec = tool.name === 'ask_user' && !tool.is_error && cedarClarify.parse(tool.output);
        if (spec) {
          const answered = state.order[state.order.length - 1] !== view.turn.id;
          const n = note('clarify', escapeHtml(tr('v2.cedarAsks')), '');
          n.lastChild.append(cedarClarify.element(spec, { answered, onSubmit: answerQuestions }));
          placeNote(view, n);
          view.stepsEl = null;
        }
        // The images a call produced are shown on the stage once each, after the reply: see finishTurn.
        // A chart drawn as a card isn't shown again.
        if (!tool.is_error && !(tool.name === 'generate_chart' && cedarChart.parse(tool.output, tool.input))) {
          for (const path of cedarImages.produced(tool.name, tool.input, tool.output)) {
            if (!view.images.includes(path)) view.images.push(path);
          }
        }
      }
      setWorking(view, tr('v2.thinking'), 'thinking');
      break;
    }
    case 'usage':
      view.usage.input += d.input_tokens || 0;
      view.usage.output += d.output_tokens || 0;
      if (typeof d.cost_usd === 'number') view.usage.cost_usd = (view.usage.cost_usd || 0) + d.cost_usd;
      return;
    default:
      if (TERMINAL.has(event.type)) { finishTurn(view, event); return; }
      return;
  }
  updateChrome();
}

/** Shown when the server stopped the reply at its output limit (`stop_reason: max_tokens`). */
const CUT_OFF = tr('v2.cutOff');

function finishTurn(view, event) {
  if (view.done) return;
  view.done = true;
  // The working line marks where this turn's notes end, so they land there even when a later turn's
  // notes are already in the column (a conversation's turns are replayed side by side).
  const anchor = view.workingEl;
  view.workingEl = null;
  const put = (node) => (anchor?.isConnected ? els.notes.insertBefore(node, anchor) : els.notes.append(node));
  const d = event.data || {};
  // Pictures the reply itself places, in its notes or in the document it wrote, are not shown again.
  const shown = new Set([...view.textNotes, view.group].flatMap((n) => [...cedarImages.shown(n)]));
  const figures = cedarImages.figuresFor(view.images, { skip: shown, onLoad: keepStageAtEnd });
  if (figures.length) placeCard(view, figuresCard(figures));
  const sources = sourcesOf(view);
  if (sources.length) { citeAll(view); placeCard(view, sourcesCard(sources)); }
  // A plan or approval still waiting when the run ended can no longer be answered.
  if (view.planEl?.dataset.state === 'awaiting') showPlan(view, view.planData, false);
  view.planEl?.cedarPlan?.finish(event.type === 'run.completed');
  for (const el of view.approvals.values()) {
    if (el.dataset.state === 'pending') el.querySelector('.cedar-approval-actions').hidden = true;
  }
  if (event.type === 'run.failed') {
    const n = note('error', '');
    n.lastChild.textContent = d.error || tr('v2.failed');
    put(n);
  } else if (event.type === 'run.cancelled') {
    put(note('muted', '', escapeHtml(d.reason === 'server shutting down' ? tr('v2.stoppedShutdown') : tr('v2.stopped'))));
  } else if (d.stop_reason === 'max_tokens') {
    put(note('error', '', escapeHtml(CUT_OFF)));
  } else if (!view.texts.length && d.stop_reason !== 'ask_user') {
    put(note('muted', '', escapeHtml(tr('v2.noText'))));
  }
  for (const tool of view.tools) {
    if (tool.output === undefined) { Object.assign(tool, { output: '', is_error: true }); fillStep(tool); fillCard(tool); }
  }
  if (view.usage.input || view.usage.output) {
    // The sandbox line only from a cedar that says how the run was sandboxed.
    const sandbox = view.started?.sandbox ? ` · ${cedarTrust.sandbox.text(view.started, view.approvalsGranted)}` : '';
    put(note('usage', '', `${cedarTrust.cost.text(view.usage)}${sandbox}`));
  }
  anchor?.remove();
  if (state.activeTurn === view) {
    state.activeTurn = null;
    // Watched to the end: not news. Hidden tabs leave it unread, for the inbox and the title.
    markSeen();
    els.notes.scrollTop = els.notes.scrollHeight;
    // Once the turn is over, the stage rests on the top of its main result: the document it wrote,
    // else its map, recipe, trip or stock card; a turn of searches and commands stays at the end.
    const main = view.tools.find((tool) => tool.card && cardKind(tool) === 'doc') || view.tools.find((tool) => tool.card && cardKind(tool) === 'widget');
    if (main) main.card.scrollIntoView({ block: 'start' });
  }
  view.lastEvent = event.type;
  updateChrome();
  renderRecents();
  if (els.filesDialog.open) loadFiles(); // the response may have added or changed files
}

// -- chrome ------------------------------------------------------------------------------------

/** Whether the open conversation has a finished response to schedule. */
function hasCompletedTurn() {
  for (const view of state.views.values()) {
    if (view.lastEvent === 'run.completed' || (!view.lastEvent && view.turn.status === 'completed')) return true;
  }
  return false;
}

function updateChrome() {
  const c = state.current;
  const busy = Boolean(state.activeTurn);
  const latest = state.views.get(state.order[state.order.length - 1]);
  els.app.classList.toggle('empty', !c);
  els.title.textContent = c ? (c.title || tr('v2.newChat')) : '';
  els.deleteChat.hidden = !c;
  document.getElementById('rename-item').hidden = !c;
  els.filesBtn.hidden = !c;
  els.scheduleBadge.hidden = !c?.schedule_id;
  cedarStar.setCurrent(c);
  els.scheduleBtn.hidden = !c || busy || state.sending || !hasCompletedTurn();
  els.stop.hidden = !busy;
  els.when.textContent = c?.created_at ? new Date(c.created_at).toLocaleDateString(cedarI18n.locale, { weekday: 'long', day: 'numeric', month: 'long' }) : '';
  els.status.className = 'status' + (busy ? ' live' : latest?.lastEvent === 'run.failed' ? ' error' : '');
  const doing = state.activeTurn?.doing || tr('v2.thinking');
  // German nouns keep their capital; English and French lower-case the step after "Working ·".
  const doingText = cedarI18n.lang === 'de' ? doing : `${doing[0].toLowerCase()}${doing.slice(1)}`;
  els.statusText.textContent = busy ? tr('v2.status.working', { doing: doingText })
    : tr(latest ? (latest.lastEvent === 'run.failed' ? 'v2.status.failed' : latest.lastEvent === 'run.cancelled' ? 'v2.status.stopped' : 'v2.status.done') : 'v2.status.idle');
  els.send.disabled = state.sending || busy || state.noModels || !els.input.value.trim();
  els.input.placeholder = state.noModels ? NO_MODELS
    : tr(busy ? 'v2.placeholder.busy' : c ? 'v2.placeholder.reply' : 'v2.hero.title');
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

// -- the two panes on a narrow screen ----------------------------------------------------------

function setPane(pane) {
  els.app.dataset.pane = pane;
  if (pane === 'stage') state.unseen = 0;
  updateTabs();
}

function updateTabs() {
  for (const tab of els.tabs.querySelectorAll('.tab')) tab.setAttribute('aria-selected', String(tab.dataset.pane === els.app.dataset.pane));
  els.stageBadge.hidden = !state.unseen;
  els.stageBadge.textContent = state.unseen > 99 ? '99+' : String(state.unseen);
}

// -- the splitter ------------------------------------------------------------------------------

const TALK_MIN = 300;
const talkMax = () => Math.max(TALK_MIN, Math.min(720, window.innerWidth - 560));

function setTalkWidth(px, save) {
  const w = Math.round(Math.min(talkMax(), Math.max(TALK_MIN, px)));
  document.documentElement.style.setProperty('--talk-w', `${w}px`);
  if (save) storage('set', SPLIT_KEY, String(w));
}

function bindSplitter() {
  const saved = Number(storage('get', SPLIT_KEY));
  if (saved) setTalkWidth(saved, false);
  let startX = 0;
  let startW = 0;
  els.splitter.addEventListener('pointerdown', (e) => {
    startX = e.clientX;
    startW = els.talk.getBoundingClientRect().width;
    els.splitter.setPointerCapture(e.pointerId);
    els.splitter.classList.add('active');
    els.app.classList.add('resizing');
  });
  els.splitter.addEventListener('pointermove', (e) => {
    if (!els.splitter.hasPointerCapture(e.pointerId)) return;
    setTalkWidth(startW + e.clientX - startX, false);
  });
  const end = (e) => {
    if (!els.splitter.hasPointerCapture(e.pointerId)) return;
    els.splitter.releasePointerCapture(e.pointerId);
    els.splitter.classList.remove('active');
    els.app.classList.remove('resizing');
    setTalkWidth(els.talk.getBoundingClientRect().width, true);
  };
  els.splitter.addEventListener('pointerup', end);
  els.splitter.addEventListener('pointercancel', end);
  els.splitter.addEventListener('dblclick', () => { document.documentElement.style.removeProperty('--talk-w'); storage('del', SPLIT_KEY); });
  els.splitter.addEventListener('keydown', (e) => {
    const step = e.key === 'ArrowLeft' ? -24 : e.key === 'ArrowRight' ? 24 : 0;
    if (!step) return;
    e.preventDefault();
    setTalkWidth(els.talk.getBoundingClientRect().width + step, true);
  });
  window.addEventListener('resize', () => {
    const w = parseInt(document.documentElement.style.getPropertyValue('--talk-w'), 10);
    if (w && w > talkMax()) setTalkWidth(w, false);
  });
}

// -- the conversations drawer ------------------------------------------------------------------

function openDrawer() {
  els.drawer.hidden = false;
  els.backdrop.hidden = false;
  els.chatsBtn.setAttribute('aria-expanded', 'true');
  renderRecents();
  (els.recents.querySelector('.conv.active') || els.recents.querySelector('.conv') || els.drawerClose).focus();
}

function closeDrawer() {
  if (els.drawer.hidden) return;
  els.drawer.hidden = true;
  els.backdrop.hidden = true;
  els.chatsBtn.setAttribute('aria-expanded', 'false');
}

function renderRecents() {
  els.recents.textContent = '';
  if (!state.conversations.length) { els.recents.append(t('div', 'empty-list', tr('v2.noConversations'))); return; }
  for (const c of state.conversations) {
    const a = h('a', 'conv');
    a.href = `#/c/${c.id}`;
    if (state.current?.id === c.id) a.classList.add('active');
    a.append(t('span', 'name', c.title || tr('v2.newChat')));
    a.title = c.title || tr('v2.newChat');
    a.classList.toggle('is-unread', Boolean(c.unread));
    if (state.current?.id === c.id && state.activeTurn) a.append(h('span', 'live'));
    else if (c.needs_answer) a.append(t('span', 'needs', tr('v2.needsAnswer')));
    else if (c.unread) a.append(h('span', 'unread'));
    const del = h('button', 'del', '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M4 7h16M9 7V4h6v3M6 7l1 13h10l1-13"/></svg>');
    del.type = 'button';
    del.setAttribute('aria-label', tr('v2.deleteNamed', { title: c.title || tr('v2.newChat') }));
    del.addEventListener('click', (e) => { e.preventDefault(); e.stopPropagation(); deleteConversation(c); });
    a.append(del);
    els.recents.append(a);
  }
}

async function loadConversations() {
  try { state.conversations = (await api('GET', '/conversations')).conversations; } catch (e) { toast(e.message); }
  renderRecents();
}

function resetView() {
  for (const view of state.views.values()) view.abort.abort();
  state.views.clear();
  state.order = [];
  state.activeTurn = null;
  state.unseen = 0;
  els.notes.textContent = '';
  els.stageBody.textContent = '';
  els.stageEmpty.hidden = false;
  updateTabs();
  if (els.filesDialog.open) els.filesDialog.close();
}

function showNewChat() {
  state.loadToken++;
  resetView();
  state.current = null;
  cedarSettings.conversationChanged();
  cedarAttach.clear();
  updateChrome();
  renderRecents();
  els.input.focus();
}

async function openConversation(id) {
  const token = ++state.loadToken;
  resetView();
  let detail;
  try { detail = await api('GET', `/conversations/${id}`); } catch (e) {
    if (token !== state.loadToken) return;
    toast(e.message);
    location.hash = '#/';
    return;
  }
  if (token !== state.loadToken) return;
  state.current = detail.conversation;
  cedarSettings.conversationChanged();
  cedarAttach.clear();
  restorePickers(detail.conversation);
  for (const turn of detail.turns) addTurn(turn);
  updateChrome();
  renderRecents();
  if (narrow.matches) setPane('talk');
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
  updateChrome();
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
  closeDrawer();
  const m = location.hash.match(/^#\/c\/([0-9a-f-]{36})$/i);
  if (m) { if (state.current?.id !== m[1]) openConversation(m[1]); } else showNewChat();
}

// -- files -------------------------------------------------------------------------------------

/** API path (without the `/api` prefix) of the conversation's file listing, or of one file. */
function filesPath(path) {
  const base = `/conversations/${state.current.id}/files`;
  return path ? `${base}/${path.split('/').map(encodeURIComponent).join('/')}` : base;
}

function formatSize(n) {
  const fixed = (x, digits) => cedarI18n.number(x, { minimumFractionDigits: digits, maximumFractionDigits: digits });
  if (n < 1024) return tr('v2.size.b', { n: fixed(n, 0) });
  if (n < 1024 * 1024) return tr('v2.size.kb', { n: fixed(n / 1024, n < 10 * 1024 ? 1 : 0) });
  return tr('v2.size.mb', { n: fixed(n / (1024 * 1024), 1) });
}

function renderFiles(listing) {
  els.filesList.textContent = '';
  if (!listing.files.length) {
    els.filesList.append(t('div', 'empty-list', tr('v2.files.empty')));
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
    const view = cedarViewer.viewButton(f.path);
    row.append(path, size);
    if (view) row.append(view);
    row.append(downloadLink(f.path));
    els.filesList.append(row);
  }
  if (listing.truncated) els.filesList.append(t('div', 'empty-list', tr('v2.files.truncated', { count: 1000 })));
}

async function loadFiles() {
  if (!state.current) return;
  const id = state.current.id;
  els.filesList.textContent = '';
  els.filesList.append(t('div', 'empty-list', tr('v2.loading')));
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

/** Fetch a workspace file with the access token. Returns the ok response, or throws. */
async function fetchWorkspaceFile(path) {
  if (!state.current) throw new Error(tr('v2.noConversationOpen'));
  let res = await fetch(`/api${filesPath(path)}`, { headers: authHeaders() });
  if (res.status === 401) { await askToken(true); res = await fetch(`/api${filesPath(path)}`, { headers: authHeaders() }); }
  if (!res.ok) { const data = await res.json().catch(() => null); throw new Error(data?.error?.message || tr('v2.fetchFileFailed', { status: String(res.status) })); }
  return res;
}

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
  } catch (e) { toast(e.message); } finally { if (button) button.disabled = false; }
}

// -- composer, rename, delete ------------------------------------------------------------------

function autosize() {
  els.input.style.height = 'auto';
  els.input.style.height = `${Math.min(els.input.scrollHeight, window.innerHeight * 0.4)}px`;
}

/** Send the answers from a question form; throws if they could not be sent, so the form stays open. */
async function answerQuestions(text) {
  if (state.activeTurn || state.sending) {
    toast(tr('v2.answerBusy'));
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
  updateChrome();
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
    const body = { content, locale: cedarI18n.locale };
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
    if (token !== state.loadToken) return true;
    if (!conversation.title) conversation.title = turn.prompt.split('\n').find((l) => l.trim())?.trim().slice(0, 60);
    state.conversations = [conversation, ...state.conversations.filter((c) => c.id !== conversation.id)];
    addTurn(turn);
    renderRecents();
    loadConversations();
    return true;
  } catch (e) {
    toast(e.message);
    return false;
  } finally {
    state.sending = false;
    updateChrome();
  }
}

async function stopActive() {
  const view = state.activeTurn;
  if (!view) return;
  els.stop.disabled = true;
  try { await api('POST', `/turns/${view.turn.id}/cancel`); } catch (e) { toast(e.message); }
  els.stop.disabled = false;
}

async function deleteConversation(c) {
  if (!(await cedarConfirm(tr('v2.confirmDelete', { title: c.title || tr('v2.newChat') }), { ok: tr('v2.delete') }))) return;
  try { await api('DELETE', `/conversations/${c.id}`); } catch (e) { toast(e.message); return; }
  state.conversations = state.conversations.filter((x) => x.id !== c.id);
  if (state.current?.id === c.id) location.hash = '#/';
  renderRecents();
}

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
    updateChrome();
    renderRecents();
  } catch (e) { toast(e.message); }
}

// -- wiring ------------------------------------------------------------------------------------

els.composer.addEventListener('submit', (e) => { e.preventDefault(); sendMessage(); });
els.input.addEventListener('input', () => { autosize(); updateChrome(); });
els.input.addEventListener('keydown', (e) => {
  if (e.key === 'Enter' && !e.shiftKey && !e.isComposing) { e.preventDefault(); sendMessage(); }
});
els.stop.addEventListener('click', stopActive);
els.newChat.addEventListener('click', () => { location.hash = '#/'; if (!state.current) els.input.focus(); });
els.deleteChat.addEventListener('click', () => state.current && deleteConversation(state.current));
els.filesBtn.addEventListener('click', openFiles);
els.scheduleBtn.addEventListener('click', () => state.current && cedarSchedule.openCreate(state.current, { model: els.model.value, reasoning: els.reasoning.value }));
$('files-refresh').addEventListener('click', loadFiles);
$('files-close').addEventListener('click', () => els.filesDialog.close());
els.chatsBtn.addEventListener('click', () => (els.drawer.hidden ? openDrawer() : closeDrawer()));
els.drawerClose.addEventListener('click', closeDrawer);
els.backdrop.addEventListener('click', closeDrawer);
$('drawer-new').addEventListener('click', () => { closeDrawer(); if (!state.current) els.input.focus(); });
els.tabs.addEventListener('click', (e) => { const tab = e.target.closest('.tab'); if (tab) setPane(tab.dataset.pane); });
els.title.addEventListener('click', startRename);
document.getElementById('rename-item').addEventListener('click', startRename); // More › Rename
els.titleInput.addEventListener('keydown', (e) => { if (e.key === 'Enter') finishRename(true); if (e.key === 'Escape') finishRename(false); });
els.titleInput.addEventListener('blur', () => finishRename(true));
document.addEventListener('click', (e) => {
  const button = e.target.closest('[data-copy-code]');
  if (button) copyText(button.closest('.code').querySelector('code').textContent, button);
});
window.addEventListener('hashchange', route);
document.addEventListener('keydown', (e) => {
  if ((e.metaKey || e.ctrlKey) && e.shiftKey && e.key.toLowerCase() === 'o') { e.preventDefault(); location.hash = '#/'; }
  if (e.key === 'Escape' && !els.drawer.hidden) { closeDrawer(); els.chatsBtn.focus(); }
});
narrow.addEventListener('change', () => { if (!narrow.matches) { state.unseen = 0; updateTabs(); } });

async function init() {
  // The language comes first: a new browser is asked for it before anything else.
  await cedarLanguage.ready;
  cedarImages.init({ fetchFile: fetchWorkspaceFile, open: cedarViewer.open, download: downloadFile });
  cedarViewer.init({ fetchFile: fetchWorkspaceFile, renderMarkdown, download: downloadFile });
  cedarSchedule.init({ api, toast, renderMarkdown });
  cedarStar.init({ api, toast, onOpen: closeDrawer });
  cedarSettings.init({
    api, toast, authHeaders,
    config: () => state.config,
    current: () => state.current,
    anchor: els.reasoning,
  });
  // The paperclip goes after the Tools button that settings.js puts after the model picker; the
  // row of chosen files sits above the message box.
  cedarAttach.init({
    form: els.composer,
    textarea: els.input,
    anchor: els.reasoning,
    before: els.input,
    toast,
  });
  bindSplitter();
  updateTabs();
  try { state.config = await (await fetch('/api/config')).json(); } catch { toast(tr('v2.unreachable')); }
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

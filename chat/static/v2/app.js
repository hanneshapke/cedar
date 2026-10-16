'use strict';

// cedar-chat v2 "Desk" ("Cedar style"). Plain JavaScript, no build step. Talks only to /api on
// this origin. The markdown renderer and the API/token helpers are the same as in /v1/app.js;
// everything from "-- desk model --" on is new. The document the agent wrote (a .md/.txt file, or
// a PDF made from Markdown) is the page; web results, command output, and other tool results are
// clippings; the conversation is the margin.

const $ = (id) => document.getElementById(id);
const els = {
  title: $('title'), titleInput: $('title-input'), when: $('when'), status: $('status'), statusText: $('status-text'),
  stop: $('stop'), newChat: $('new-chat'), deleteChat: $('delete-chat'),
  filesBtn: $('files-btn'), filesDialog: $('files-dialog'), filesList: $('files-list'),
  scheduleBtn: $('schedule-btn'), scheduleBadge: $('schedule-badge'),
  notes: $('notes'), recents: $('recents'),
  page: $('page'), pageHead: $('page-head'), pageName: $('page-name'), pageBody: $('page-body'),
  pile: $('pile'), trail: $('trail'),
  composer: $('composer'), input: $('input'), send: $('send'), model: $('model'), reasoning: $('reasoning'),
  toast: $('toast'), tokenDialog: $('token-dialog'), tokenForm: $('token-form'),
  tokenInput: $('token-input'), tokenMessage: $('token-message'),
};

const TERMINAL = new Set(['run.completed', 'run.failed', 'run.cancelled']);
const ACTIVE = new Set(['queued', 'running']);
const TOKEN_KEY = 'cedar-chat-token';
const DOC_RE = /\.(md|markdown|txt)$/i;
const NO_MODELS = 'No model is registered yet: an admin adds one under Settings → Model server & API keys.';

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
  showAllClips: false,
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
  } catch { /* token then lasts for this page only */ }
  return null;
}

let memoryToken = storage('get');

async function copyText(text, button) {
  try {
    await navigator.clipboard.writeText(text);
    if (button) {
      const old = button.textContent;
      button.textContent = 'Copied';
      setTimeout(() => { button.textContent = old; }, 1500);
    }
  } catch {
    toast('Could not copy to the clipboard');
  }
}

const fmtMs = (ms) => (typeof ms !== 'number' ? '' : ms < 1000 ? `${ms} ms` : `${(ms / 1000).toFixed(1)} s`);

// -- markdown (same renderer as /app.js) -------------------------------------------------------

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
    `<button type="button" class="copy-btn" data-copy-code>Copy</button></div>` +
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
const authHeaders = () => (memoryToken ? { Authorization: `Bearer ${memoryToken}` } : {});
async function api(method, path, body) {
  const headers = authHeaders();
  if (body !== undefined) headers['Content-Type'] = 'application/json';
  const res = await fetch(`/api${path}`, { method, headers, body: body === undefined ? undefined : JSON.stringify(body) });
  if (res.status === 401) { await askToken(true); return api(method, path, body); }
  if (res.status === 204) return null;
  const data = await res.json().catch(() => null);
  if (!res.ok) throw new Error(data?.error?.message || `Request failed (${res.status})`);
  return data;
}

// -- desk model --------------------------------------------------------------------------------
// Each turn is a view: the prompt, the assistant's texts, and its tool calls in order. The desk
// (page, pile, trail) is re-derived from all views whenever an event arrives.

function workspacePath(path) {
  if (typeof path !== 'string') return null;
  let p = path.trim();
  if (p.startsWith('/workspace/')) p = p.slice('/workspace/'.length);
  else if (p.startsWith('./')) p = p.slice(2);
  if (!p || p.startsWith('/') || p.split('/').some((part) => !part || part === '.' || part === '..')) return null;
  return p;
}

// How each tool reads in the trail ("Ran ls") and in the "working" label ("Running ls"), and
// what its one-line subject is. Tools not listed here fall back to their bare name.
const firstLine = (s) => String(s || '').split('\n').find((l) => l.trim()) || '';
const TOOLS = {
  web_search: { doing: 'Searching', did: 'Searched', subject: (i) => `“${i.query || ''}”` },
  bash: { doing: 'Running', did: 'Ran', subject: (i) => firstLine(i.command) },
  run_python: { doing: 'Running', did: 'Ran', subject: (i) => firstLine(i.code) },
  read_file: { doing: 'Reading', did: 'Read', subject: (i) => i.path },
  read_document: { doing: 'Reading', did: 'Read', subject: (i) => i.path },
  fetch_url: { doing: 'Fetching', did: 'Fetched', subject: (i) => i.url },
  download_file: { doing: 'Downloading', did: 'Downloaded', subject: (i) => (i.path ? `${i.url} → ${i.path}` : i.url) },
  write_file: { doing: 'Writing', did: 'Wrote', subject: (i) => i.path },
  generate_pdf: { doing: 'Writing', did: 'Wrote', subject: (i) => i.path },
  generate_chart: { doing: 'Drawing', did: 'Drew', subject: (i) => i.path },
  weather_forecast: {
    doing: 'Checking the weather for',
    did: 'Checked the weather for',
    subject: (i) => i.location || (i.latitude != null ? `${i.latitude}, ${i.longitude}` : ''),
  },
  show_map: { doing: 'Mapping', did: 'Mapped', subject: (i) => mapSubject(i) },
};

/** What a show_map call is about: its title, else the places it names. */
function mapSubject(input) {
  if (input.title) return input.title;
  const places = Array.isArray(input.markers) ? input.markers : [];
  return places.map((m) => m?.location || m?.label || `${m?.latitude}, ${m?.longitude}`).join(' · ');
}
// Tools whose `path` input is a file they write, so a finished call gets a download button.
const FILE_TOOLS = new Set(['write_file', 'generate_pdf', 'generate_chart', 'download_file']);

function toolText(tool, done) {
  const t = TOOLS[tool.name];
  if (!t) return done ? tool.name : `Running ${tool.name}`;
  return `${done ? t.did : t.doing} ${t.subject(tool.input || {}) || ''}`.trim();
}

/** A tool call that produces the page: a Markdown/text file, or a PDF made from Markdown. */
function isDocument(tool) {
  const { path, format } = tool.input || {};
  if (typeof path !== 'string') return false;
  if (tool.name === 'write_file') return DOC_RE.test(path);
  return tool.name === 'generate_pdf' && (!format || format === 'markdown');
}

/** The newest document written across all turns, or null. */
function currentDocument() {
  for (let t = state.order.length - 1; t >= 0; t--) {
    const view = state.views.get(state.order[t]);
    for (let i = view.tools.length - 1; i >= 0; i--) {
      const tool = view.tools[i];
      if (isDocument(tool) && tool.is_error !== true) return tool;
    }
  }
  return null;
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

function clipEl(kind, meta, headHtml, subHtml, pre, isError) {
  const el = h('div', `clip${isError ? ' err' : ''}`);
  el.append(h('div', 'kind', `<span>Clipping · ${escapeHtml(kind)}</span><span class="ms">${escapeHtml(meta)}</span>`));
  if (headHtml) el.append(h('div', 'head', headHtml));
  if (subHtml) el.append(h('div', 'sub', subHtml));
  if (pre !== undefined) { const p = h('pre'); p.textContent = pre; el.append(p); }
  return el;
}

function searchClippings(tool, meta) {
  const query = tool.input?.query || '';
  if (tool.output === undefined) {
    const el = clipEl('web_search', meta, escapeHtml(query), 'searching…');
    el.querySelector('.sub').prepend(h('span', 'spinner'), ' ');
    return [el];
  }
  if (tool.is_error) return [clipEl('web_search', meta, escapeHtml(query), '', tool.output, true)];
  const results = parseSearchResults(tool.output);
  if (!results.length) return [clipEl('web_search', meta, escapeHtml(query), escapeHtml(tool.output || 'No results.'))];
  return results.map((r) => clipEl('web_search', meta, linkHtml(r.url, r.title),
    [hostOf(r.url), r.snippet, r.age].filter(Boolean).map(escapeHtml).join(' · ')));
}

/**
 * The clipping for a show_map call: the map itself once the result is in. A map is a live widget, so
 * it is built once and the same element is moved into each new clipping when the desk is redrawn.
 */
function mapClipping(tool, meta) {
  const head = escapeHtml(mapSubject(tool.input || {}));
  if (tool.output === undefined) {
    const el = clipEl('show_map', meta, head, 'locating…');
    el.querySelector('.sub').prepend(h('span', 'spinner'), ' ');
    return el;
  }
  if (tool.is_error) return clipEl('show_map', meta, head, '', tool.output, true);
  const spec = cedarMap.parse(tool.output);
  if (!spec) return clipEl('show_map', meta, head, '', tool.output);
  tool.mapEl ??= cedarMap.element(spec);
  // The map's own card shows the title, so the clipping's head only counts the pins.
  const n = spec.markers.length;
  const el = clipEl('show_map', meta, `${n} place${n === 1 ? '' : 's'}`, '');
  el.classList.add('map');
  el.append(tool.mapEl);
  return el;
}

/** Clippings for one tool call; web_search gives one per result, and the page's document gives none. */
function clippingsFor(tool) {
  if (isDocument(tool) && tool.is_error !== true) return [];
  const input = tool.input || {};
  const done = tool.output !== undefined;
  const meta = done ? fmtMs(tool.duration_ms) : 'running';
  const shell = done ? (String(tool.output).trim() || '(no output)') : undefined;
  const raw = done ? String(tool.output) : undefined;
  switch (tool.name) {
    case 'web_search':
      return searchClippings(tool, meta);
    case 'bash':
      return [clipEl('bash', meta, `<code>$ ${escapeHtml(firstLine(input.command))}</code>`, '', shell, tool.is_error)];
    case 'run_python': {
      return [clipEl('run_python', meta, `<code>${escapeHtml(firstLine(input.code))}</code>`, '', shell, tool.is_error)];
    }
    case 'read_file':
    case 'read_document':
      return [clipEl(tool.name, meta, escapeHtml(String(input.path || '')), '', raw, tool.is_error)];
    case 'fetch_url':
      return [clipEl('fetch_url', meta, linkHtml(String(input.url || '')), '', raw, tool.is_error)];
    case 'show_map':
      return [mapClipping(tool, meta)];
    default:
      if (FILE_TOOLS.has(tool.name)) {
        const path = String(input.path || '');
        const saved = done && !tool.is_error;
        const el = clipEl(tool.name, meta, escapeHtml(path), saved ? 'saved to the workspace' : '', tool.is_error ? String(tool.output) : undefined, tool.is_error);
        const rel = saved && workspacePath(path);
        if (rel) {
          const view = cedarViewer.viewButton(rel);
          if (view) el.querySelector('.sub').append(' · ', view);
          el.querySelector('.sub').append(' · ', downloadLink(rel));
        }
        return [el];
      }
      // A tool this page doesn't know yet: still show what it was given and what it returned.
      return [clipEl(tool.name, meta, `<code>${escapeHtml(JSON.stringify(input).slice(0, 120))}</code>`, '', shell, tool.is_error)];
  }
}

function downloadLink(path, primary) {
  const b = h('button', `copy-btn${primary ? ' primary' : ''}`, 'Download');
  b.type = 'button';
  b.title = `Download ${path}`;
  b.addEventListener('click', () => downloadFile(path, b));
  return b;
}

/** Rendered Markdown for the page, with the workspace images it refers to. */
function pageMarkdown(text) {
  const md = h('div', 'md', renderMarkdown(text));
  cedarImages.hydrate(md);
  return md;
}

function renderDesk() {
  // page
  const doc = currentDocument();
  const latest = state.views.get(state.order[state.order.length - 1]);
  els.page.classList.toggle('drafting', Boolean(doc && doc.output === undefined));
  els.pageHead.textContent = '';
  els.pageBody.textContent = '';
  if (doc) {
    const path = String(doc.input.path);
    const name = path.split('/').pop();
    const written = doc.output !== undefined;
    const size = doc.name === 'generate_pdf' ? 'PDF' : `${new Blob([doc.input.content || '']).size.toLocaleString()} B`;
    els.pageHead.append(h('span', 'page-name', escapeHtml(name)), h('span', '', written ? `written · ${escapeHtml(fmtMs(doc.duration_ms))} · ${size}` : 'being written…'));
    const actions = h('div', 'actions');
    const copy = h('button', 'copy-btn', 'Copy');
    copy.type = 'button';
    copy.addEventListener('click', () => copyText(doc.input.content || '', copy));
    actions.append(copy);
    const rel = written && workspacePath(path);
    if (rel) {
      // The page shows a PDF's Markdown source; the finished PDF opens in the viewer.
      const view = doc.name === 'generate_pdf' ? cedarViewer.viewButton(rel, { label: 'View PDF' }) : null;
      if (view) actions.append(view);
      actions.append(downloadLink(rel, true));
    }
    els.pageHead.append(actions);
    const content = String(doc.input.content ?? '');
    if (/\.txt$/i.test(path)) { const pre = h('pre', 'plain'); pre.textContent = content; els.pageBody.append(pre); }
    else els.pageBody.append(pageMarkdown(content));
  } else if (latest && latest.texts.length) {
    els.pageHead.append(h('span', 'page-name', 'Answer'), h('span', '', 'no file was written; this is what Cedar said'));
    els.pageBody.append(pageMarkdown(latest.texts.join('\n\n')));
  } else if (latest) {
    els.pageHead.append(h('span', 'page-name', 'Working'));
    const w = h('div', 'working');
    w.append(h('span', 'spinner'), 'Nothing on the desk yet.');
    els.pageBody.append(w);
  } else {
    els.pageHead.append(h('span', 'page-name', 'Nothing on the desk yet'));
    els.pageBody.append(h('div', 'page-empty', '<div class="slot">your first file lands here</div><p>Give Cedar a task. It reads, writes, and leaves the results on the desk.</p>'));
  }

  // pile: the latest turn's clippings, newest on top; earlier turns behind a count
  els.pile.textContent = '';
  els.trail.textContent = '';
  if (!latest) return;
  const turnsShown = state.showAllClips ? state.order : state.order.slice(-1);
  const clips = [];
  for (const id of turnsShown) for (const tool of state.views.get(id).tools) clips.push(...clippingsFor(tool));
  clips.reverse();
  for (const c of clips) els.pile.append(c);
  const earlier = state.order.length - 1;
  if (earlier > 0) {
    const more = h('button', 'clip more');
    more.type = 'button';
    more.textContent = state.showAllClips ? 'Show only this turn' : `${earlier} earlier turn${earlier > 1 ? 's' : ''} of clippings`;
    more.addEventListener('click', () => { state.showAllClips = !state.showAllClips; renderDesk(); });
    els.pile.append(more);
  }

  // trail: the latest turn's steps
  if (latest.tools.length) {
    els.trail.append(h('div', 'label', 'Trail'));
    latest.tools.forEach((tool, i) => {
      const done = tool.output !== undefined;
      const row = h('div', `step${!done ? ' pending' : tool.is_error ? ' err' : ''}`);
      const what = toolText(tool, done);
      row.append(h('span', '', String(i + 1)), h('span', 'path', escapeHtml(what)), h('span', 'ms', done ? (tool.is_error ? 'failed' : fmtMs(tool.duration_ms)) : 'running'));
      row.querySelector('.path').title = what;
      els.trail.append(row);
    });
  }
}

// -- margin ------------------------------------------------------------------------------------

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
    workingEl: null,
  };
  state.views.set(turn.id, view);
  state.order.push(turn.id);
  const user = note('user', `You · ${new Date(turn.created_at || Date.now()).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}`);
  if (turn.attachments?.length) user.lastChild.append(cedarAttach.chips(turn.attachments));
  user.lastChild.append(document.createTextNode(turn.prompt));
  els.notes.append(user);
  view.workingEl = h('div', 'working');
  view.workingEl.append(h('span', 'spinner'), h('span', '', 'Thinking'));
  els.notes.append(view.workingEl);
  if (ACTIVE.has(turn.status)) state.activeTurn = view;
  updateChrome();
  followEvents(view);
  return view;
}

/** After a picture in the margin loads: stay at the end if the reader was there, else leave them be. */
function keepNotesAtEnd() {
  const n = els.notes;
  if (n.scrollHeight - n.scrollTop - n.clientHeight < 160) n.scrollTop = n.scrollHeight;
}

function placeNote(view, el) {
  els.notes.insertBefore(el, view.workingEl);
  els.notes.scrollTop = els.notes.scrollHeight;
}

function setWorking(view, text) {
  if (view.workingEl) view.workingEl.lastChild.textContent = text;
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
      if (e.fatal || ++failures > 5) { finishTurn(view, { type: 'run.failed', data: { error: e.message } }); return; }
    }
    if (!view.done) await sleep(Math.min(1000 * 2 ** failures, 8000));
  }
}

function applyEvent(view, event) {
  const d = event.data || {};
  switch (event.type) {
    case 'assistant.text':
      view.texts.push(d.text);
      view.textNotes.push(note('cedar', 'Cedar', renderMarkdown(d.text)));
      placeNote(view, view.textNotes[view.textNotes.length - 1]);
      setWorking(view, 'Thinking');
      break;
    case 'tool.call': {
      const tool = { id: d.id, name: d.name, input: d.input || {} };
      view.tools.push(tool);
      view.byId.set(d.id, tool);
      setWorking(view, toolText(tool, false));
      break;
    }
    case 'tool.result': {
      const tool = view.byId.get(d.id);
      if (tool) {
        Object.assign(tool, { output: String(d.output ?? ''), is_error: Boolean(d.is_error), duration_ms: d.duration_ms });
        // The images a call produced are shown in the margin once each, after the reply: see finishTurn.
        if (!tool.is_error) {
          for (const path of cedarImages.produced(tool.name, tool.input, tool.output)) {
            if (!view.images.includes(path)) view.images.push(path);
          }
        }
      }
      setWorking(view, 'Thinking');
      break;
    }
    case 'usage':
      view.usage.input += d.input_tokens || 0;
      view.usage.output += d.output_tokens || 0;
      return;
    default:
      if (TERMINAL.has(event.type)) { finishTurn(view, event); return; }
      return;
  }
  renderDesk();
  updateChrome();
}

/** Shown when the server stopped the reply at its output limit (`stop_reason: max_tokens`). */
const CUT_OFF =
  "The reply was cut off at the model's output limit (MAX_TOKENS). A model that thinks before answering can use all of it before writing any text. Ask whoever runs the server to raise it.";

function finishTurn(view, event) {
  if (view.done) return;
  view.done = true;
  // The working line marks where this turn's notes end, so they land there even when a later turn's
  // notes are already in the margin (a conversation's turns are replayed side by side).
  const anchor = view.workingEl;
  view.workingEl = null;
  const put = (node) => (anchor?.isConnected ? els.notes.insertBefore(node, anchor) : els.notes.append(node));
  const d = event.data || {};
  const inText = new Set(view.textNotes.flatMap((n) => [...cedarImages.shown(n)])); // pictures the reply itself places
  const figures = cedarImages.figuresFor(view.images, { skip: inText, onLoad: keepNotesAtEnd });
  if (figures.length) {
    const n = note('cedar', '', '');
    n.lastChild.append(...figures);
    put(n);
  }
  if (event.type === 'run.failed') {
    const n = note('error', '');
    n.lastChild.textContent = d.error || 'Something went wrong.';
    put(n);
  } else if (event.type === 'run.cancelled') {
    put(note('muted', '', d.reason === 'server shutting down' ? 'Stopped: the server shut down.' : 'Stopped.'));
  } else if (d.stop_reason === 'max_tokens') {
    put(note('error', '', CUT_OFF));
  } else if (!view.texts.length) {
    put(note('muted', '', 'No text in this response.'));
  }
  for (const tool of view.tools) if (tool.output === undefined) Object.assign(tool, { output: '', is_error: true });
  if (view.usage.input || view.usage.output) {
    put(note('usage', '', `${view.usage.input.toLocaleString()} in · ${view.usage.output.toLocaleString()} out`));
  }
  anchor?.remove();
  els.notes.scrollTop = els.notes.scrollHeight;
  view.lastEvent = event.type;
  if (state.activeTurn === view) state.activeTurn = null;
  renderDesk();
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
  els.title.textContent = c ? (c.title || 'New chat') : '';
  els.deleteChat.hidden = !c;
  els.filesBtn.hidden = !c;
  els.scheduleBadge.hidden = !c?.schedule_id;
  els.scheduleBtn.hidden = !c || busy || state.sending || !hasCompletedTurn();
  els.stop.hidden = !busy;
  els.when.textContent = c?.created_at ? new Date(c.created_at).toLocaleDateString([], { weekday: 'long', day: 'numeric', month: 'long' }) : '';
  els.status.className = 'status' + (busy ? ' live' : latest?.lastEvent === 'run.failed' ? ' error' : '');
  const doing = state.activeTurn?.workingEl?.lastChild.textContent || 'Thinking';
  els.statusText.textContent = busy ? `Working · ${doing[0].toLowerCase()}${doing.slice(1)}`
    : latest ? (latest.lastEvent === 'run.failed' ? 'Failed' : latest.lastEvent === 'run.cancelled' ? 'Stopped' : 'Done') : 'Idle';
  els.send.disabled = state.sending || busy || state.noModels || !els.input.value.trim();
  els.input.placeholder = state.noModels ? NO_MODELS
    : busy ? 'Cedar is working. Type a follow-up for when it is done…' : c ? 'Change anything?' : 'What should be on the desk?';
  document.title = c?.title ? `${c.title} – Cedar` : 'Cedar';
}

function renderRecents() {
  els.recents.textContent = '';
  if (!state.conversations.length) { els.recents.append(h('div', 'empty-list', 'No conversations yet')); return; }
  for (const c of state.conversations) {
    const a = h('a');
    a.href = `#/c/${c.id}`;
    if (state.current?.id === c.id) a.classList.add('active');
    if (state.current?.id === c.id && state.activeTurn) a.append(h('span', 'live'));
    a.append(document.createTextNode(c.title || 'New chat'));
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
  state.showAllClips = false;
  els.notes.textContent = '';
  if (els.filesDialog.open) els.filesDialog.close();
}

function showNewChat() {
  state.loadToken++;
  resetView();
  state.current = null;
  cedarSettings.conversationChanged();
  cedarAttach.clear();
  renderDesk();
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
  renderDesk();
  updateChrome();
  renderRecents();
  els.input.focus();
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
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(n < 10 * 1024 ? 1 : 0)} KB`;
  return `${(n / (1024 * 1024)).toFixed(1)} MB`;
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
    const view = cedarViewer.viewButton(f.path);
    row.append(path, size);
    if (view) row.append(view);
    row.append(downloadLink(f.path));
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

/** Fetch a workspace file with the access token. Returns the ok response, or throws. */
async function fetchWorkspaceFile(path) {
  if (!state.current) throw new Error('No conversation is open.');
  let res = await fetch(`/api${filesPath(path)}`, { headers: authHeaders() });
  if (res.status === 401) { await askToken(true); res = await fetch(`/api${filesPath(path)}`, { headers: authHeaders() }); }
  if (!res.ok) { const data = await res.json().catch(() => null); throw new Error(data?.error?.message || `Could not fetch the file (${res.status})`); }
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

async function sendMessage() {
  const content = els.input.value.trim();
  if (!content || state.sending || state.activeTurn) return;
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
    const body = { content };
    if (els.model.value) body.model = els.model.value;
    if (els.reasoning.value) body.reasoning = els.reasoning.value;
    const attachments = cedarAttach.payload();
    if (attachments.length) body.attachments = attachments;
    const turn = await api('POST', `/conversations/${conversation.id}/messages`, body);
    els.input.value = '';
    cedarAttach.clear(); // only once sent: a failed send keeps the files, like the text
    autosize();
    if (token !== state.loadToken) return;
    if (!conversation.title) conversation.title = turn.prompt.split('\n').find((l) => l.trim())?.trim().slice(0, 60);
    state.conversations = [conversation, ...state.conversations.filter((c) => c.id !== conversation.id)];
    addTurn(turn);
    renderDesk();
    renderRecents();
    loadConversations();
  } catch (e) {
    toast(e.message);
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
  if (!confirm(`Delete "${c.title || 'New chat'}"? This cannot be undone.`)) return;
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
els.title.addEventListener('click', startRename);
els.titleInput.addEventListener('keydown', (e) => { if (e.key === 'Enter') finishRename(true); if (e.key === 'Escape') finishRename(false); });
els.titleInput.addEventListener('blur', () => finishRename(true));
document.addEventListener('click', (e) => {
  const button = e.target.closest('[data-copy-code]');
  if (button) copyText(button.closest('.code').querySelector('code').textContent, button);
});
window.addEventListener('hashchange', route);
document.addEventListener('keydown', (e) => {
  if ((e.metaKey || e.ctrlKey) && e.shiftKey && e.key.toLowerCase() === 'o') { e.preventDefault(); location.hash = '#/'; }
});

async function init() {
  cedarImages.init({ fetchFile: fetchWorkspaceFile, open: cedarViewer.open, download: downloadFile });
  cedarViewer.init({ fetchFile: fetchWorkspaceFile, renderMarkdown, download: downloadFile });
  cedarSchedule.init({ api, toast, renderMarkdown });
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
  try { state.config = await (await fetch('/api/config')).json(); } catch { toast('Could not reach the server'); }
  sendMenu = cedarModels.sendMenu({ send: els.send, modelSelect: els.model, levelSelect: els.reasoning });
  if (state.config.auth_required && !memoryToken) await askToken(false);
  await loadModels();
  cedarSettings.start();
  await loadConversations();
  route();
}

init();

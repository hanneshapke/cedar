// The contribution site: read an export in the browser, let the person pick and review one
// conversation, and send exactly the bundle they reviewed. Building the bundle (import, the
// redaction scan, the consistency checks) is the WebAssembly engine's job (contrib-web/src);
// this file is the page around it. Nothing here sends anything except `send()`, the policy read
// and the receipts' status and withdraw calls.

import { readZip } from './zip.js';

const $ = (id) => document.getElementById(id);
const config = window.CEDAR_CONTRIB_CONFIG || {};

// The ingest: the configured one, or a local one named in ?ingest= for development (a link
// can't point a person's contribution at a stranger's server).
function ingestUrl() {
  const param = new URLSearchParams(location.search).get('ingest');
  if (param && /^http:\/\/(localhost|127\.0\.0\.1)(:\d+)?$/.test(param)) return param;
  return (config.ingest || location.origin).replace(/\/+$/, '');
}
const INGEST = ingestUrl();

// -- storage that may not be there (private windows, blocked site data) ---------------------
const store = {
  get(key, fallback) {
    try { const v = localStorage.getItem(key); return v == null ? fallback : JSON.parse(v); } catch { return fallback; }
  },
  set(key, value) {
    try { localStorage.setItem(key, JSON.stringify(value)); return true; } catch { return false; }
  },
};

// -- the engine -------------------------------------------------------------------------------
let engine = null;
async function loadEngine() {
  const url = 'cedar_contrib_web.wasm';
  let instance;
  try {
    ({ instance } = await WebAssembly.instantiateStreaming(fetch(url), {}));
  } catch {
    const bytes = await (await fetch(url)).arrayBuffer();
    ({ instance } = await WebAssembly.instantiate(bytes, {}));
  }
  engine = instance.exports;
}

function build(request) {
  const input = new TextEncoder().encode(JSON.stringify(request));
  const ptr = engine.cw_alloc(input.length);
  new Uint8Array(engine.memory.buffer, ptr, input.length).set(input);
  const out = engine.cw_build(ptr, input.length);
  // Memory may have grown during the call: read the buffer afresh.
  const len = new DataView(engine.memory.buffer).getUint32(out, true);
  const text = new TextDecoder().decode(new Uint8Array(engine.memory.buffer, out + 4, len));
  engine.cw_free(out, len + 4);
  return JSON.parse(text);
}

function randomHex(bytes) {
  const b = crypto.getRandomValues(new Uint8Array(bytes));
  return Array.from(b, (x) => x.toString(16).padStart(2, '0')).join('');
}

// -- state ------------------------------------------------------------------------------------
const state = {
  conversations: [], // {source, title, day, exchanges, raw}
  current: null,
  seed: null,
  preview: null,
  policy: null,
  ratings: {},
  phrases: [],
};

// -- step 1: the export -----------------------------------------------------------------------
function setStatus(el, text, kind) {
  el.textContent = text;
  el.className = 'status' + (kind ? ' ' + kind : '');
}

function sourceOf(c) {
  if (c && typeof c === 'object' && c.mapping && typeof c.mapping === 'object') return 'chatgpt';
  if (c && typeof c === 'object' && Array.isArray(c.chat_messages)) return 'claude';
  return null;
}

function summarize(c) {
  const source = sourceOf(c);
  if (source === 'chatgpt') {
    const msgs = Object.values(c.mapping).filter((n) => n && n.message && n.message.author && n.message.author.role === 'user');
    const t = c.create_time ? new Date(c.create_time * 1000) : null;
    return { source, title: c.title || 'Untitled', day: t ? t.toISOString().slice(0, 10) : '', prompts: msgs.length, raw: c };
  }
  const prompts = c.chat_messages.filter((m) => m.sender === 'human').length;
  return { source, title: (c.name || '').trim() || 'Untitled', day: (c.created_at || '').slice(0, 10), prompts, raw: c };
}

async function loadFile(file) {
  const status = $('load-status');
  setStatus(status, `Reading ${file.name}…`);
  try {
    let texts;
    const head = new Uint8Array(await file.slice(0, 4).arrayBuffer());
    if (head[0] === 0x50 && head[1] === 0x4b) {
      const entries = await readZip(file, (name) => /(^|\/)conversations(-\d+)?\.json$/.test(name) && !name.startsWith('__MACOSX'));
      if (!entries.length) throw new Error('No conversations.json in this zip. Is it a ChatGPT or Claude export?');
      texts = entries.map((e) => e.text);
    } else {
      texts = [await file.text()];
    }
    const all = [];
    for (const text of texts) {
      const data = JSON.parse(text);
      const list = Array.isArray(data) ? data : [data];
      for (const c of list) if (sourceOf(c)) all.push(summarize(c));
    }
    if (!all.length) throw new Error('No ChatGPT or Claude conversations in this file.');
    all.sort((a, b) => (b.day || '').localeCompare(a.day || ''));
    state.conversations = all;
    const kinds = new Set(all.map((c) => c.source));
    setStatus(status, `Found ${all.length} conversation${all.length === 1 ? '' : 's'} from ${[...kinds].map(label).join(' and ')}. Nothing has left your browser.`, 'ok');
    $('step-pick').hidden = false;
    renderList();
    $('step-pick').scrollIntoView({ behavior: 'smooth', block: 'start' });
  } catch (e) {
    setStatus(status, e.message || String(e), 'error');
  }
}

function label(source) {
  return source === 'chatgpt' ? 'ChatGPT' : 'Claude';
}

// -- step 2: the list -------------------------------------------------------------------------
const LIST_MAX = 300;
function renderList() {
  const q = $('filter').value.trim().toLowerCase();
  const list = $('convs');
  list.replaceChildren();
  const hits = state.conversations.filter((c) => !q || c.title.toLowerCase().includes(q));
  for (const c of hits.slice(0, LIST_MAX)) {
    const li = document.createElement('li');
    const b = document.createElement('button');
    b.type = 'button';
    if (state.current === c) b.setAttribute('aria-current', 'true');
    const title = document.createElement('span');
    title.className = 'conv-title';
    title.textContent = c.title;
    const badge = document.createElement('span');
    badge.className = 'badge';
    badge.textContent = label(c.source);
    const meta = document.createElement('span');
    meta.className = 'conv-meta';
    meta.textContent = [c.day, `${c.prompts} prompt${c.prompts === 1 ? '' : 's'}`].filter(Boolean).join(' · ');
    b.append(title, badge, meta);
    b.addEventListener('click', () => pick(c));
    li.append(b);
    list.append(li);
  }
  $('convs-more').textContent = hits.length > LIST_MAX ? `Showing ${LIST_MAX} of ${hits.length}. Search to narrow it down.` : hits.length ? '' : 'No titles match.';
}

function pick(c) {
  state.current = c;
  state.seed = randomHex(8);
  state.ratings = {};
  state.phrases = [];
  $('upto').value = 50;
  $('opt-reasoning').checked = false;
  $('opt-fetched').checked = false;
  $('notes').value = '';
  $('consent').checked = false;
  $('model-input').value = '';
  for (const r of document.querySelectorAll('input[name="outcome"]')) r.checked = false;
  $('language').dataset.touched = '';
  setStatus($('send-status'), '');
  renderList();
  $('step-review').hidden = false;
  rebuild();
  $('step-review').scrollIntoView({ behavior: 'smooth', block: 'start' });
}

// -- step 3: the review -----------------------------------------------------------------------
function options() {
  const outcome = document.querySelector('input[name="outcome"]:checked');
  const usePseudonym = $('use-pseudonym').checked;
  const languageTouched = $('language').dataset.touched === '1';
  return {
    reasoning: $('opt-reasoning').checked,
    fetched: $('opt-fetched').checked,
    upto: Math.max(1, Math.min(50, parseInt($('upto').value, 10) || 50)),
    model: $('model-input').value.trim() || null,
    language: languageTouched ? $('language').value : null,
    license: document.querySelector('input[name="license"]:checked').value,
    contributor: usePseudonym ? pseudonym() : null,
    outcome: outcome ? { result: outcome.value, notes: $('notes').value.trim() || undefined } : null,
    ratings: state.ratings,
    phrases: state.phrases,
    models_allowed: state.policy && Array.isArray(state.policy.models) ? state.policy.models : null,
    max_bundle_bytes: state.policy && state.policy.max_bundle_bytes ? state.policy.max_bundle_bytes : null,
    today: new Date().toISOString().slice(0, 10),
  };
}

let rebuildTimer = 0;
function rebuildSoon() {
  clearTimeout(rebuildTimer);
  rebuildTimer = setTimeout(rebuild, 180);
}

function rebuild() {
  if (!state.current || !engine) return;
  const out = build({ conversation: state.current.raw, seed: state.seed, options: options() });
  if (out.error) {
    state.preview = null;
    $('transcript').replaceChildren(text('p', out.error, 'status error'));
    updateSend();
    return;
  }
  state.preview = out.ok;
  render();
}

function text(tag, content, cls) {
  const el = document.createElement(tag);
  el.textContent = content;
  if (cls) el.className = cls;
  return el;
}

// Text with every stand-in marked, so the person sees what the scan replaced.
function marked(str, standins) {
  const frag = document.createDocumentFragment();
  if (!standins.size || !str) { frag.append(str || ''); return frag; }
  const re = new RegExp([...standins.keys()].sort((a, b) => b.length - a.length).map((s) => s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')).join('|'), 'g');
  let at = 0;
  for (const m of str.matchAll(re)) {
    frag.append(str.slice(at, m.index));
    const mark = document.createElement('mark');
    mark.className = 'standin';
    mark.textContent = m[0];
    mark.title = `Stand-in (${standins.get(m[0])}): not the original`;
    frag.append(mark);
    at = m.index + m[0].length;
  }
  frag.append(str.slice(at));
  return frag;
}

function pre(value, standins) {
  const p = document.createElement('pre');
  p.append(marked(typeof value === 'string' ? value : JSON.stringify(value, null, 2), standins));
  return p;
}

const KIND_NAMES = { EMAIL: 'email addresses', PHONE: 'phone numbers', IBAN: 'bank accounts', CARD: 'card numbers', IP: 'IP addresses', USER: 'user names', FILE: 'file names', KEY: 'keys and passwords', TOKEN: 'URL tokens', CUSTOM: 'your phrases' };
const BLOCK_REASONS = {
  empty: () => 'This conversation has no answered prompts.',
  private_tools: (p) => `It used tools that read your accounts (${p.private_tools.join(', ')}). That's other people's data, so it can't be contributed.`,
  secrets: (p) => `A secret is still in the text (${p.secrets.join('; ')}). Replace it with a phrase redaction.`,
  not_consistent: (p) => `It doesn't pass the corpus's checks: ${p.problems.slice(0, 3).join('; ')}`,
  too_large: () => 'It is larger than the corpus takes. Include fewer exchanges.',
  model_not_allowed: (p) => `This corpus doesn't publish outputs of ${p.bundle.model.name}, so it can't be sent here.`,
};

function render() {
  const p = state.preview;
  const b = p.bundle;
  const standins = new Map();
  for (const [kind, values] of Object.entries(b.redactions || {})) for (const v of values) standins.set(v, kind);

  // The conversation: the last turn holds all of it. Reasoning sits on the calls, by the index
  // of the assistant message it came before.
  const reasoning = new Map();
  for (const t of b.turns) for (const c of t.calls) if (c.reasoning) reasoning.set(c.message_count, c.reasoning);
  const box = $('transcript');
  box.replaceChildren();
  const last = b.turns[b.turns.length - 1];
  let exchange = 0;
  const messages = last ? last.messages : [];
  messages.forEach((m, i) => {
    const isPrompt = m.role === 'user' && !m.content.some((c) => c.type === 'tool_result');
    if (isPrompt && exchange > 0) box.append(rateBar(exchange));
    if (isPrompt) exchange++;
    if (reasoning.has(i)) {
      const d = document.createElement('details');
      d.className = 'tool';
      d.append(text('summary', 'Reasoning'), pre(reasoning.get(i), standins));
      box.append(d);
    }
    const div = document.createElement('div');
    div.className = 'msg ' + (m.role === 'assistant' ? 'assistant' : isPrompt ? 'user' : 'tool-msg');
    const who = isPrompt ? 'You' : m.role === 'assistant' ? label(state.current.source) : 'Tool results';
    div.append(text('span', who, 'who'));
    for (const c of m.content) {
      if (c.type === 'text') div.append(marked(c.text, standins));
      else if (c.type === 'image' || c.type === 'document') div.append(text('span', `${c.type === 'image' ? 'Image' : 'File'} left out${c.source && c.source.bytes ? `, ${fmtBytes(c.source.bytes)}` : ''}`, 'chip'));
      else if (c.type === 'tool_use') {
        const d = document.createElement('details');
        d.className = 'tool';
        d.append(text('summary', `Called ${c.name}`), pre(c.input, standins));
        div.append(d);
      } else if (c.type === 'tool_result') {
        const d = document.createElement('details');
        d.className = 'tool';
        d.append(text('summary', `Result${c.is_error ? ' (error)' : ''}`), pre(c.content, standins));
        div.append(d);
      }
    }
    box.append(div);
  });
  if (exchange > 0) box.append(rateBar(exchange));
  $('raw-pre').textContent = JSON.stringify(JSON.parse(p.json), null, 2);

  // Panel.
  $('upto-total').textContent = String(p.exchanges_total);
  $('upto').max = String(Math.min(50, Math.max(1, p.exchanges_total)));
  if ($('language').dataset.touched !== '1') $('language').value = p.language_guess;
  if (![...$('language').options].some((o) => o.value === b.language)) $('language').append(new Option(b.language, b.language));
  $('model-line').textContent = b.model.name;
  $('model-pick').hidden = p.model_known;

  const counts = {};
  for (const [kind, values] of Object.entries(b.redactions || {})) counts[kind] = values.length;
  const sl = $('standins');
  sl.replaceChildren();
  const kinds = Object.keys(counts);
  if (!kinds.length) sl.append(text('li', 'The scan found nothing with a known shape.'));
  for (const k of kinds) sl.append(text('li', `${counts[k]} ${KIND_NAMES[k] || k} replaced`));

  const pl = $('phrases');
  pl.replaceChildren();
  state.phrases.forEach((ph, i) => {
    const li = document.createElement('li');
    li.append(text('span', `“${ph.phrase}” → ${ph.replacement || 'a neutral stand-in'}`));
    const rm = text('button', 'Remove');
    rm.type = 'button';
    rm.addEventListener('click', () => { state.phrases.splice(i, 1); rebuild(); });
    li.append(rm);
    pl.append(li);
  });

  const d = b.dropped || {};
  const left = [];
  if (d.attachments) left.push(`${d.attachments} file${d.attachments === 1 ? '' : 's'} and image${d.attachments === 1 ? '' : 's'}`);
  if (d.fetched_bodies) left.push(`${d.fetched_bodies} fetched page${d.fetched_bodies === 1 ? '' : 's'}`);
  if (d.reasoning) left.push(`reasoning in ${d.reasoning} answer${d.reasoning === 1 ? '' : 's'}`);
  if (d.memory) left.push('your custom instructions and memories');
  $('dropped').textContent = left.length ? `Always left out or not chosen: ${left.join(', ')}. The title is never sent.` : 'The title is never sent.';

  const v = $('verdict');
  v.replaceChildren();
  if (p.blocked.length) {
    v.className = 'verdict blocked';
    v.append(text('strong', "This conversation can't be sent yet"));
    const ul = document.createElement('ul');
    for (const r of p.blocked) ul.append(text('li', (BLOCK_REASONS[r] || (() => r))(p)));
    v.append(ul);
  } else {
    v.className = 'verdict ok';
    v.append(text('strong', 'Ready once you have read it.'));
    v.append(text('div', `${p.exchanges_included} exchange${p.exchanges_included === 1 ? '' : 's'}, ${fmtBytes(p.size_bytes)}${state.policy ? '' : '. The corpus could not be reached to check its rules; it checks them when you send.'}`));
  }
  updateSend();
}

function rateBar(seq) {
  const bar = document.createElement('div');
  bar.className = 'rate';
  bar.append(text('span', 'This answer:'));
  for (const [val, sym, name] of [[1, '👍', 'Good answer'], [-1, '👎', 'Bad answer']]) {
    const btn = text('button', sym);
    btn.type = 'button';
    btn.setAttribute('aria-label', name);
    btn.setAttribute('aria-pressed', String(state.ratings[seq] === val));
    btn.addEventListener('click', () => {
      if (state.ratings[seq] === val) delete state.ratings[seq];
      else state.ratings[seq] = val;
      rebuild();
    });
    bar.append(btn);
  }
  return bar;
}

function fmtBytes(n) {
  if (n < 1024) return `${n} bytes`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / 1024 / 1024).toFixed(1)} MB`;
}

function updateSend() {
  const ready = state.preview && !state.preview.blocked.length && $('consent').checked;
  $('send').disabled = !ready;
}

// Selecting text in the conversation offers to redact it.
function selectionToRedact() {
  const pop = $('redact-pop');
  const sel = window.getSelection();
  const s = sel && sel.rangeCount ? sel.toString().trim() : '';
  if (!s || s.length > 200 || !$('transcript').contains(sel.anchorNode)) { pop.hidden = true; return; }
  const rect = sel.getRangeAt(0).getBoundingClientRect();
  const col = document.querySelector('.transcript-col').getBoundingClientRect();
  pop.style.top = `${rect.bottom - col.top + 6}px`;
  pop.style.left = `${Math.max(0, Math.min(rect.left - col.left, col.width - 160))}px`;
  pop.dataset.text = s;
  pop.textContent = `Redact “${s.length > 24 ? s.slice(0, 24) + '…' : s}”`;
  pop.hidden = false;
}

function addPhrase(phrase, replacement) {
  phrase = phrase.trim();
  if (phrase.length < 2 || state.phrases.some((p) => p.phrase === phrase)) return;
  state.phrases.push({ phrase, replacement: (replacement || '').trim() || null });
  rebuild();
}

// -- pseudonym --------------------------------------------------------------------------------
function pseudonym() {
  let p = store.get('cedar-contrib.pseudonym', null);
  if (!p) { p = 'p_' + randomHex(8); store.set('cedar-contrib.pseudonym', p); }
  return p;
}

function renderPseudonym() {
  const line = $('pseudonym-line');
  line.replaceChildren();
  if (!$('use-pseudonym').checked) {
    line.textContent = 'Unlinked: this contribution stands alone and cannot be tied to your others.';
    return;
  }
  line.append(`Your pseudonym is ${pseudonym()}, random and kept in this browser. `);
  const a = text('a', 'Make a new one');
  a.href = '#';
  a.addEventListener('click', (e) => {
    e.preventDefault();
    store.set('cedar-contrib.pseudonym', 'p_' + randomHex(8));
    renderPseudonym();
    rebuild();
  });
  line.append(a);
}

// -- sending ----------------------------------------------------------------------------------
async function loadPolicy() {
  try {
    const r = await fetch(`${INGEST}/v1/policy`, { credentials: 'omit' });
    if (r.ok) state.policy = await r.json();
  } catch { /* the review says the rules couldn't be read; the ingest checks on send */ }
  if (state.current) rebuild();
}

const SEND_ERRORS = {
  schema: 'The corpus could not read it',
  not_consistent: 'The corpus found it inconsistent',
  secret_detected: 'The corpus found a secret in it',
  model_not_allowed: "The corpus doesn't take this model's outputs",
  duplicate: 'This conversation was already contributed',
  quota: "You've reached the limit for now",
  rate_limited: 'Too many requests; try again in a minute',
  too_large: 'It is too large',
};

async function send() {
  const p = state.preview;
  if (!p || p.blocked.length || !$('consent').checked) return;
  const status = $('send-status');
  $('send').disabled = true;
  setStatus(status, 'Sending…');
  try {
    const r = await fetch(`${INGEST}/v1/contributions`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: p.json, credentials: 'omit' });
    const body = await r.json().catch(() => ({}));
    if (r.status !== 201) {
      const code = body.error && body.error.code;
      throw new Error(`${SEND_ERRORS[code] || `The corpus refused it (${r.status})`}${body.error && body.error.message ? `: ${body.error.message}` : ''}`);
    }
    const receipts = store.get('cedar-contrib.receipts', []);
    receipts.unshift({
      receipt_id: body.receipt_id,
      withdraw_token: body.withdraw_token,
      status: body.status || 'quarantined',
      title: state.current.title,
      source: state.current.source,
      model: p.bundle.model.name,
      sent: new Date().toISOString().slice(0, 10),
      ingest: INGEST,
    });
    const kept = store.set('cedar-contrib.receipts', receipts);
    setStatus(status, kept ? 'Sent. Thank you! Your receipt is below.' : 'Sent. Thank you! This browser could not keep the receipt, so download it now if you may want to withdraw.', 'ok');
    if (!kept) download(`cedar-receipt-${body.receipt_id}.json`, JSON.stringify([receipts[0]], null, 2));
    renderReceipts();
  } catch (e) {
    setStatus(status, e.message || String(e), 'error');
    updateSend();
  }
}

// -- receipts ---------------------------------------------------------------------------------
function renderReceipts() {
  const list = $('receipt-list');
  list.replaceChildren();
  const receipts = store.get('cedar-contrib.receipts', []);
  if (!receipts.length) list.append(text('li', 'Nothing sent from this browser yet.'));
  for (const r of receipts) {
    const li = document.createElement('li');
    const main = document.createElement('div');
    main.className = 'r-main';
    main.append(text('div', r.title || r.receipt_id, 'r-title'), text('div', `${label(r.source)} · ${r.model} · sent ${r.sent} · ${r.status}${r.released_in ? ` in ${r.released_in}` : ''}`, 'r-meta'));
    li.append(main);
    if (r.status !== 'withdrawn') {
      const w = text('button', 'Withdraw');
      w.type = 'button';
      w.addEventListener('click', () => withdraw(r.receipt_id));
      li.append(w);
    }
    list.append(li);
  }
}

async function refreshReceipts() {
  const receipts = store.get('cedar-contrib.receipts', []);
  for (const r of receipts) {
    if (r.status === 'withdrawn') continue;
    try {
      const res = await fetch(`${r.ingest || INGEST}/v1/contributions/${encodeURIComponent(r.receipt_id)}`, { headers: { authorization: `Bearer ${r.withdraw_token}` }, credentials: 'omit' });
      if (res.ok) {
        const s = await res.json();
        r.status = s.status;
        r.released_in = Array.isArray(s.released_in) ? s.released_in.join(', ') : s.released_in || '';
      }
    } catch { /* keep what we had */ }
  }
  store.set('cedar-contrib.receipts', receipts);
  renderReceipts();
}

async function withdraw(id) {
  if (!confirm('Withdraw this contribution? It is removed from the queue and from future releases.')) return;
  const receipts = store.get('cedar-contrib.receipts', []);
  const r = receipts.find((x) => x.receipt_id === id);
  if (!r) return;
  try {
    const res = await fetch(`${r.ingest || INGEST}/v1/contributions/${encodeURIComponent(id)}`, { method: 'DELETE', headers: { authorization: `Bearer ${r.withdraw_token}` }, credentials: 'omit' });
    if (res.status !== 204) throw new Error(`The corpus answered ${res.status}`);
    r.status = 'withdrawn';
    store.set('cedar-contrib.receipts', receipts);
    renderReceipts();
  } catch (e) {
    alert(`Could not withdraw: ${e.message}`);
  }
}

function download(name, content) {
  const url = URL.createObjectURL(new Blob([content], { type: 'application/json' }));
  const a = document.createElement('a');
  a.href = url;
  a.download = name;
  document.body.append(a);
  a.click();
  a.remove();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}

// -- wiring -----------------------------------------------------------------------------------
function wire() {
  const drop = $('drop');
  $('file').addEventListener('change', (e) => e.target.files[0] && loadFile(e.target.files[0]));
  drop.addEventListener('dragover', (e) => { e.preventDefault(); drop.classList.add('over'); });
  drop.addEventListener('dragleave', () => drop.classList.remove('over'));
  drop.addEventListener('drop', (e) => {
    e.preventDefault();
    drop.classList.remove('over');
    if (e.dataTransfer.files[0]) loadFile(e.dataTransfer.files[0]);
  });
  $('filter').addEventListener('input', renderList);

  for (const id of ['opt-reasoning', 'opt-fetched']) $(id).addEventListener('change', rebuild);
  for (const r of document.querySelectorAll('input[name="outcome"], input[name="license"]')) r.addEventListener('change', rebuild);
  $('upto').addEventListener('input', rebuildSoon);
  $('model-input').addEventListener('input', rebuildSoon);
  $('notes').addEventListener('input', rebuildSoon);
  $('language').addEventListener('change', () => { $('language').dataset.touched = '1'; rebuild(); });
  $('use-pseudonym').addEventListener('change', () => {
    store.set('cedar-contrib.use-pseudonym', $('use-pseudonym').checked);
    renderPseudonym();
    rebuild();
  });
  $('consent').addEventListener('change', updateSend);
  $('send').addEventListener('click', send);
  $('phrase-form').addEventListener('submit', (e) => {
    e.preventDefault();
    addPhrase($('phrase').value, $('replacement').value);
    $('phrase').value = '';
    $('replacement').value = '';
  });
  document.addEventListener('selectionchange', () => setTimeout(selectionToRedact, 0));
  $('redact-pop').addEventListener('mousedown', (e) => e.preventDefault());
  $('redact-pop').addEventListener('click', () => {
    addPhrase($('redact-pop').dataset.text || '');
    $('redact-pop').hidden = true;
    window.getSelection().removeAllRanges();
  });

  const tab = (raw) => {
    $('tab-conv').setAttribute('aria-selected', String(!raw));
    $('tab-raw').setAttribute('aria-selected', String(raw));
    $('transcript').hidden = raw;
    $('raw').hidden = !raw;
  };
  // The landing page's export instructions: one service at a time.
  for (const svc of ['chatgpt', 'claude']) {
    $(`howtab-${svc}`).addEventListener('click', () => {
      for (const other of ['chatgpt', 'claude']) {
        $(`howtab-${other}`).setAttribute('aria-selected', String(other === svc));
        $(`howto-${other}`).hidden = other !== svc;
      }
    });
  }

  $('tab-conv').addEventListener('click', () => tab(false));
  $('tab-raw').addEventListener('click', () => tab(true));
  $('download').addEventListener('click', (e) => {
    e.preventDefault();
    if (state.preview) download('cedar-contribution.json', state.preview.json);
  });

  $('refresh-receipts').addEventListener('click', refreshReceipts);
  $('export-receipts').addEventListener('click', () => download('cedar-receipts.json', JSON.stringify(store.get('cedar-contrib.receipts', []), null, 2)));
  $('import-receipts').addEventListener('change', async (e) => {
    const f = e.target.files[0];
    if (!f) return;
    try {
      const incoming = JSON.parse(await f.text());
      const have = store.get('cedar-contrib.receipts', []);
      for (const r of Array.isArray(incoming) ? incoming : [incoming]) {
        if (r && r.receipt_id && r.withdraw_token && !have.some((h) => h.receipt_id === r.receipt_id)) have.push(r);
      }
      store.set('cedar-contrib.receipts', have);
      renderReceipts();
    } catch {
      alert("That file isn't a receipts file.");
    }
  });
}

async function main() {
  $('ingest-host').textContent = new URL(INGEST).host;
  $('use-pseudonym').checked = store.get('cedar-contrib.use-pseudonym', false) === true;
  renderPseudonym();
  renderReceipts();
  wire();
  try {
    await loadEngine();
  } catch (e) {
    setStatus($('load-status'), `This browser could not start the page's engine (${e.message}). A current Chrome, Firefox, Safari or Edge works.`, 'error');
    $('file').disabled = true;
    return;
  }
  loadPolicy();
}

main();

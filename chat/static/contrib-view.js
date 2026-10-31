'use strict';

// Contributing a conversation to a training corpus, shared by every UI (see
// docs/design/contributions.md and chat/src/contrib.rs). Two pieces:
//
//   - The review dialog, from the More menu's "Contribute this conversation…" (or "Export for
//     contribution…" when this server has no corpus to send to). It shows the bundle exactly as
//     it would be sent (`GET /api/conversations/{id}/contribution`), with every placeholder the
//     redaction left marked, lets the person include reasoning or fetched pages, redact more
//     phrases, rate each response, say how it went, download the JSON, and Send: the document in
//     the "Exactly what is sent" tab is what goes (`POST .../contribution`).
//   - Settings → Contribute: the one-time choices (license, pseudonym, defaults), the switch
//     that turns contributing on, and the receipts with Withdraw.
//
// Each app hands over its helpers through cedarContrib.init():
//
//   api(method, path, body) -> Promise<json>   the app's /api client (sends the access token)
//   toast(message)                             a short notice
//   config() -> object                         /api/config (contributions: whether a corpus is set)
//   current() -> conversation | null           the open conversation
//
// The dialog draws with trace-view.css's pieces (the same dialog frame, chips, folds and
// preformatted blocks), plus the few of its own in contrib-view.css.

const cedarContrib = (() => {
  const PLACEHOLDER = /\[([A-Z]{2,12})_(\d{1,6})\]/g;
  const TABS = ['review', 'raw'];

  let deps = null;
  let dialog = null;
  let current = null;   // { cid, opts, preview, bundle, custom: [], outcome, notes, tab, loading, error, sending }
  let settingsData = null;  // /api/settings/contributions, as last loaded
  let receipts = null;      // /api/contributions, as last loaded

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

  function chip(text, kind = '') {
    return el('span', `cedar-trace-chip ${kind}`.trim(), text);
  }

  function note(text, kind = '') {
    return el('p', `cedar-trace-note ${kind}`.trim(), text);
  }

  function json(value) {
    return JSON.stringify(value, null, 2);
  }

  /** Text with every `[KIND_n]` placeholder marked. */
  function marked(text) {
    const frag = document.createDocumentFragment();
    let at = 0;
    for (const m of String(text ?? '').matchAll(PLACEHOLDER)) {
      frag.append(document.createTextNode(text.slice(at, m.index)));
      const mark = el('mark', 'cedar-contrib-ph', m[0]);
      mark.title = m[1];
      frag.append(mark);
      at = m.index + m[0].length;
    }
    frag.append(document.createTextNode(String(text ?? '').slice(at)));
    return frag;
  }

  function pre(text, className = '') {
    const p = el('pre', `cedar-trace-pre ${className}`.trim());
    p.append(marked(text));
    return p;
  }

  function fold(summaryNodes, bodyNodes, { open = false, className = '' } = {}) {
    const d = el('details', `cedar-trace-fold ${className}`.trim());
    d.open = open;
    const s = el('summary');
    s.append(...summaryNodes);
    const body = el('div', 'cedar-trace-fold-body');
    body.append(...bodyNodes);
    d.append(s, body);
    return d;
  }

  function shell() {
    return typeof cedarSettingsShell === 'undefined' ? null : cedarSettingsShell;
  }

  function expert() {
    return typeof cedarTrace !== 'undefined' && cedarTrace.enabled();
  }

  // -- the bundle as it will be sent -------------------------------------------------------------

  /** Every string of the conversation's text in `bundle`, through `f` (in place). */
  function walkText(bundle, f) {
    const walk = (v) => {
      if (typeof v === 'string') return f(v);
      if (Array.isArray(v)) return v.map(walk);
      if (v && typeof v === 'object') { for (const k of Object.keys(v)) v[k] = walk(v[k]); return v; }
      return v;
    };
    for (const t of bundle.turns || []) {
      t.prompt = f(t.prompt);
      t.system = f(t.system);
      t.messages = (t.messages || []).map((m) => ({ ...m, content: walk(m.content) }));
      for (const c of t.calls || []) {
        if (typeof c.reasoning === 'string') c.reasoning = f(c.reasoning);
        if (typeof c.error === 'string') c.error = f(c.error);
      }
      t.events = walk(t.events || []);
    }
    if (bundle.outcome?.notes) bundle.outcome.notes = f(bundle.outcome.notes);
  }

  /** The server's bundle with the person's own redactions and outcome applied: what Send posts. */
  function finalBundle() {
    if (!current?.preview) return null;
    const bundle = JSON.parse(JSON.stringify(current.preview.bundle));
    if (current.outcome) bundle.outcome = { result: current.outcome, ...(current.notes.trim() ? { notes: current.notes.trim() } : {}) };
    else delete bundle.outcome;
    const phrases = current.custom.filter((p) => p.trim());
    if (phrases.length) {
      walkText(bundle, (s) => {
        let out = s;
        phrases.forEach((p, i) => { if (out.includes(p)) out = out.split(p).join(`[CUSTOM_${i + 1}]`); });
        return out;
      });
      bundle.redactions = { ...(bundle.redactions || {}), CUSTOM: phrases.length };
    }
    return bundle;
  }

  function counts(bundle) {
    return Object.values(bundle?.redactions || {}).reduce((n, v) => n + v, 0);
  }

  // -- the dialog --------------------------------------------------------------------------------

  function ensureDialog() {
    if (dialog) return dialog;
    dialog = el('dialog', 'cedar-trace cedar-contrib');
    dialog.setAttribute('aria-labelledby', 'cedar-contrib-title');
    dialog.innerHTML = `
      <div class="cedar-trace-head">
        <div class="cedar-trace-heading">
          <h3 id="cedar-contrib-title"></h3>
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

  async function load() {
    if (!current) return;
    current.loading = true;
    current.error = null;
    render();
    const q = [];
    if (current.opts.reasoning) q.push('reasoning=true');
    if (current.opts.fetched) q.push('fetched=true');
    try {
      current.preview = await deps.api('GET', `/conversations/${encodeURIComponent(current.cid)}/contribution${q.length ? `?${q.join('&')}` : ''}`);
    } catch (err) {
      current.error = err.message;
    } finally {
      current.loading = false;
      render();
    }
  }

  function summaryChips() {
    const p = current.preview;
    if (!p) return [];
    const b = p.bundle;
    const d = b.dropped || {};
    const parts = [
      chip(tr('contrib.summary.turns', { count: (b.turns || []).length })),
      chip(b.model?.name || ''),
      chip(tr('contrib.summary.redactions', { count: counts(finalBundle()) }), 'accent'),
      d.memory ? chip(tr('contrib.summary.memoryCut')) : null,
      d.attachments ? chip(tr('contrib.summary.attachments', { count: d.attachments })) : null,
      d.fetched_bodies ? chip(tr('contrib.summary.fetched', { count: d.fetched_bodies })) : null,
      d.reasoning ? chip(tr('contrib.summary.reasoning', { count: d.reasoning })) : null,
      p.blocked?.length ? chip(tr('contrib.summary.blocked'), 'error') : chip(tr('contrib.summary.ready'), 'ok'),
    ];
    return parts.filter(Boolean);
  }

  function render() {
    if (!dialog || !current) return;
    dialog.querySelector('#cedar-contrib-title').textContent = tr('contrib.title');
    const summary = dialog.querySelector('.cedar-trace-summary');
    summary.replaceChildren(...summaryChips());

    const actions = dialog.querySelector('.cedar-trace-actions');
    actions.textContent = '';
    const refresh = btn(tr('contrib.refresh'), load);
    refresh.disabled = current.loading;
    actions.append(refresh);
    const p = current.preview;
    if (p) {
      actions.append(btn(tr('contrib.download'), download));
      if (p.can_send) {
        const send = btn(current.sending ? tr('contrib.sending') : tr('contrib.send'), sendNow, 'cedar-trace-btn cedar-contrib-send');
        send.disabled = current.sending || !p.accepted || (p.blocked || []).length > 0;
        actions.append(send);
      }
    }
    const close = btn('×', () => dialog.close(), 'cedar-trace-btn close');
    close.setAttribute('aria-label', tr('contrib.close'));
    actions.append(close);

    const tabs = dialog.querySelector('.cedar-trace-tabs');
    tabs.textContent = '';
    for (const id of TABS) {
      const b = btn(tr(`contrib.tab.${id}`), () => { current.tab = id; render(); }, 'cedar-trace-tab');
      b.setAttribute('role', 'tab');
      b.setAttribute('aria-selected', String(current.tab === id));
      tabs.append(b);
    }

    const body = dialog.querySelector('.cedar-trace-body');
    body.textContent = '';
    if (current.loading && !p) { body.append(note(tr('contrib.loading'))); return; }
    if (current.error) body.append(pre(current.error, 'error'));
    if (!p) return;
    body.append(...(current.tab === 'raw' ? renderRaw() : renderReview()));
  }

  function renderRaw() {
    const b = finalBundle();
    const p = el('pre', 'cedar-trace-pre raw');
    p.textContent = json(b);
    return [note(tr('contrib.rawNote')), p];
  }

  function renderReview() {
    const p = current.preview;
    const out = [];
    // Why it can't go, if it can't.
    for (const reason of p.blocked || []) {
      const params = { tools: (p.private_tools || []).join(', '), model: p.bundle?.model?.name || '' };
      out.push(note(tr(`contrib.blocked.${reason}`, params), 'warn'));
      if (reason === 'secrets') out.push(...(p.secrets || []).map((s) => pre(s, 'error')));
      if (reason === 'not_consistent') out.push(...(p.problems || []).map((s) => pre(s, 'error')));
    }
    if (!p.can_send) out.push(note(tr('contrib.exportOnly')));
    else if (!p.accepted) {
      const n = note(tr('contrib.turnOn'));
      n.append(' ', btn(tr('contrib.openSettings'), () => { dialog.close(); shell()?.go('contribute'); }, 'cedar-trace-btn small'));
      out.push(n);
    }
    out.push(note(tr('contrib.intro')));

    // What to include.
    const options = el('div', 'cedar-contrib-options');
    for (const [key, label] of [['reasoning', tr('contrib.option.reasoning')], ['fetched', tr('contrib.option.fetched')]]) {
      const l = el('label', 'cedar-contrib-check');
      const c = el('input');
      c.type = 'checkbox';
      c.checked = !!current.opts[key];
      c.addEventListener('change', () => { current.opts[key] = c.checked; load(); });
      l.append(c, el('span', '', label));
      options.append(l);
    }
    out.push(options);

    // Redact more.
    const redact = el('div', 'cedar-contrib-redact');
    const label = el('label', 'cedar-trace-label', tr('contrib.redact.label'));
    const input = el('input', 'cedar-contrib-input');
    input.type = 'text';
    input.placeholder = tr('contrib.redact.placeholder');
    input.setAttribute('aria-label', tr('contrib.redact.label'));
    const add = () => {
      const phrase = input.value.trim();
      if (phrase.length < 2 || current.custom.includes(phrase)) return;
      current.custom.push(phrase);
      input.value = '';
      render();
    };
    input.addEventListener('keydown', (e) => { if (e.key === 'Enter') { e.preventDefault(); add(); } });
    redact.append(label, input, btn(tr('contrib.redact.add'), add, 'cedar-trace-btn small'));
    for (const [i, phrase] of current.custom.entries()) {
      const c = chip(`[CUSTOM_${i + 1}] = ${phrase}`, 'accent');
      const x = btn('×', () => { current.custom.splice(i, 1); render(); }, 'cedar-contrib-x');
      x.setAttribute('aria-label', tr('contrib.redact.remove'));
      c.append(' ', x);
      redact.append(c);
    }
    out.push(redact);

    // The outcome.
    const outcome = el('div', 'cedar-contrib-outcome');
    outcome.append(el('div', 'cedar-trace-label', tr('contrib.outcome.title')));
    const row = el('div', 'cedar-contrib-row');
    for (const result of ['succeeded', 'partly', 'failed']) {
      const b = btn(tr(`contrib.outcome.${result}`), () => { current.outcome = current.outcome === result ? null : result; render(); }, 'cedar-trace-btn small');
      b.setAttribute('aria-pressed', String(current.outcome === result));
      row.append(b);
    }
    const notes = el('textarea', 'cedar-contrib-notes');
    notes.rows = 2;
    notes.placeholder = tr('contrib.outcome.notes');
    notes.value = current.notes;
    // No re-render on change: a blur that rebuilt the dialog would swallow the click that caused it.
    notes.addEventListener('input', () => { current.notes = notes.value; });
    outcome.append(row, notes);
    out.push(outcome);

    // The turns, as they will be sent.
    const bundle = finalBundle();
    for (const t of bundle.turns || []) out.push(turnCard(t));
    return out;
  }

  function turnCard(t) {
    const card = el('section', 'cedar-contrib-turn');
    const head = el('div', 'cedar-contrib-turn-head');
    head.append(el('strong', '', tr('contrib.turn', { n: t.seq })), chip(t.status, t.status === 'completed' ? 'ok' : 'error'), el('span', 'cedar-trace-dim', t.day));
    const rate = el('div', 'cedar-contrib-rate');
    for (const [value, label, glyph] of [[1, tr('contrib.rate.good'), '👍'], [-1, tr('contrib.rate.bad'), '👎']]) {
      const b = btn(glyph, () => rateTurn(t.seq, t.rating === value ? null : value), 'cedar-contrib-thumb');
      b.title = label;
      b.setAttribute('aria-label', label);
      b.setAttribute('aria-pressed', String(t.rating === value));
      rate.append(b);
    }
    head.append(rate);
    card.append(head);

    const user = el('div', 'cedar-trace-msg user');
    user.append(chip(tr('contrib.you'), 'user'), pre(t.prompt));
    card.append(user);
    const messages = (t.messages || []).slice(1);
    const agent = el('div', 'cedar-trace-msg assistant');
    agent.append(chip(tr('contrib.agent'), 'assistant'));
    for (const m of messages) {
      for (const b of m.content || []) {
        switch (b.type) {
          case 'text':
            agent.append(pre(b.text || ''));
            break;
          case 'tool_use':
            agent.append(fold([chip('tool_use', 'tool'), el('span', 'cedar-trace-ellipsis mono', b.name)], [pre(json(b.input ?? {}))], { className: 'tool' }));
            break;
          case 'tool_result': {
            const text = typeof b.content === 'string' ? b.content : json(b.content);
            const first = text.split('\n')[0].slice(0, 120);
            agent.append(fold([chip('tool_result', b.is_error ? 'error' : 'tool'), el('span', 'cedar-trace-ellipsis', first)], [pre(text)], { className: 'tool' }));
            break;
          }
          case 'image':
          case 'document':
            agent.append(el('div', 'cedar-trace-label', `${b.type} · ${b.source?.media_type || ''} · ${tr('contrib.fileLeftOut')}`));
            break;
          default:
            agent.append(pre(json(b)));
        }
      }
    }
    for (const c of t.calls || []) {
      if (c.reasoning) agent.append(fold([chip('reasoning', 'accent'), el('span', 'cedar-trace-dim', `#${c.seq}`)], [pre(c.reasoning)], { className: 'event' }));
    }
    for (const e of t.events || []) {
      if (!e.type.startsWith('run.')) agent.append(fold([chip(e.type, 'tool')], [pre(json(e.data))], { className: 'event' }));
    }
    card.append(agent);
    return card;
  }

  async function rateTurn(seq, rating) {
    const turn = current?.preview?.bundle?.turns?.[seq - 1];
    const id = current?.preview?.turn_ids?.[seq - 1];
    if (!turn || !id) return;
    try {
      await deps.api('PUT', `/turns/${encodeURIComponent(id)}/rating`, { rating });
      turn.rating = rating;
      render();
    } catch (err) {
      deps.toast(tr('contrib.rateFailed', { error: err.message }));
    }
  }

  function download() {
    const bundle = finalBundle();
    const blob = new Blob([json(bundle)], { type: 'application/json' });
    const url = URL.createObjectURL(blob);
    const a = el('a');
    a.href = url;
    a.download = `cedar-contribution-${current.cid}.json`;
    document.body.append(a);
    a.click();
    a.remove();
    setTimeout(() => URL.revokeObjectURL(url), 1000);
  }

  async function sendNow() {
    if (!current?.preview || current.sending) return;
    const bundle = finalBundle();
    current.sending = true;
    render();
    try {
      const receipt = await deps.api('POST', `/conversations/${encodeURIComponent(current.cid)}/contribution`, { bundle, outcome: bundle.outcome || null });
      deps.toast(tr('contrib.sent', { id: receipt.id }));
      receipts = null;
      dialog.close();
    } catch (err) {
      current.error = tr('contrib.sendFailed', { error: err.message });
    } finally {
      current.sending = false;
      render();
    }
  }

  /** Open the review dialog for a conversation. */
  function open(cid) {
    ensureDialog();
    if (current?.cid !== cid) current = { cid, opts: { reasoning: false, fetched: false }, preview: null, custom: [], outcome: null, notes: '', tab: 'review' };
    if (!dialog.open) dialog.showModal();
    load();
  }

  // -- the More menu item ------------------------------------------------------------------------

  function menuItem() {
    return document.getElementById('contribute-item');
  }

  function refreshMenu() {
    const item = menuItem();
    if (!item || !deps) return;
    const c = deps.current();
    const corpus = !!deps.config()?.contributions;
    item.hidden = !c || !(corpus || expert());
    const label = item.querySelector('span');
    if (label) label.textContent = tr(corpus ? 'contrib.menu' : 'contrib.menuExport');
  }

  function bindMenu() {
    const item = menuItem();
    if (!item) return;
    item.addEventListener('click', () => { const c = deps.current(); if (c) open(c.id); });
    for (const toggle of document.querySelectorAll('[data-more-toggle]')) {
      toggle.addEventListener('click', refreshMenu, true);
    }
    refreshMenu();
  }

  // -- Settings → Contribute ---------------------------------------------------------------------

  async function saveSettings(section, patch) {
    try {
      settingsData = await deps.api('PUT', '/settings/contributions', patch);
    } catch (err) {
      deps.toast(tr('contrib.settings.saveFailed', { error: err.message }));
    }
    drawSettings(section);
    shell()?.refresh('contribute');
  }

  function switchRow(title, desc, on, onChange) {
    const row = el('div', 'cedar-set-integration');
    const main = el('div', 'cedar-set-integration-main');
    main.append(el('strong', '', title), el('small', '', desc));
    const side = el('div', 'cedar-set-integration-side');
    const sw = el('button', 'cedar-set-switch');
    sw.type = 'button';
    sw.setAttribute('role', 'switch');
    sw.setAttribute('aria-checked', String(on));
    sw.setAttribute('aria-label', title);
    sw.addEventListener('click', () => onChange(!on));
    side.append(sw);
    row.append(main, side);
    return row;
  }

  function drawSettings(section) {
    const s = settingsData;
    const parts = [];
    if (!s) return;
    if (!s.available) {
      parts.push(el('p', 'cedar-set-note', tr('contrib.settings.unavailable')));
    }
    // What a contribution is.
    const help = el('div', 'cedar-set-help');
    if (s.host) help.append(el('p', 'cedar-set-note', tr('contrib.settings.corpus', { host: s.host })));
    help.append(el('span', 'settings-eyebrow', tr('contrib.settings.what')));
    const ul = el('ul');
    for (const k of ['what1', 'what2', 'what3', 'what4']) ul.append(el('li', '', tr(`contrib.settings.${k}`)));
    help.append(ul);
    parts.push(help);

    if (s.available) {
      const block = el('div', 'cedar-set-block');
      // License.
      const lic = el('div', 'cedar-set-integration');
      const licMain = el('div', 'cedar-set-integration-main');
      licMain.append(el('strong', '', tr('contrib.settings.license')));
      const select = el('select', 'cedar-contrib-select');
      for (const id of s.licenses || []) {
        const o = el('option', '', tr(id === 'cc0' ? 'contrib.settings.licenseCc0' : 'contrib.settings.licenseCcBy'));
        o.value = id;
        o.selected = id === s.license;
        select.append(o);
      }
      select.addEventListener('change', () => saveSettings(section, { license: select.value }));
      const licSide = el('div', 'cedar-set-integration-side');
      licSide.append(select);
      lic.append(licMain, licSide);
      block.append(lic);
      // Pseudonym.
      const ps = el('div', 'cedar-set-integration');
      const psMain = el('div', 'cedar-set-integration-main');
      psMain.append(el('strong', '', tr('contrib.settings.pseudonym')), el('small', '', tr('contrib.settings.pseudonymDesc')));
      const psSide = el('div', 'cedar-set-integration-side');
      psSide.append(el('code', 'cedar-contrib-code', s.pseudonym || tr('contrib.settings.unlinked')));
      psSide.append(btn(tr('contrib.settings.newPseudonym'), () => saveSettings(section, { pseudonym: 'new' }), 'settings-ghost'));
      if (s.pseudonym) psSide.append(btn(tr('contrib.settings.dropPseudonym'), () => saveSettings(section, { pseudonym: '' }), 'settings-ghost'));
      ps.append(psMain, psSide);
      block.append(ps);
      block.append(switchRow(tr('contrib.settings.reasoning'), tr('contrib.settings.reasoningDesc'), !!s.include_reasoning, (on) => saveSettings(section, { include_reasoning: on })));
      block.append(switchRow(tr('contrib.settings.fetched'), tr('contrib.settings.fetchedDesc'), !!s.include_fetched, (on) => saveSettings(section, { include_fetched: on })));
      block.append(switchRow(tr('contrib.settings.enabled'), tr('contrib.settings.enabledDesc'), !!s.accepted_at, (on) => saveSettings(section, { enabled: on })));
      parts.push(block);
    }

    // Receipts.
    const rec = el('div', 'cedar-set-block');
    rec.append(el('span', 'settings-eyebrow', tr('contrib.settings.receipts')));
    const list = receipts?.contributions || [];
    if (!list.length) rec.append(el('p', 'cedar-set-note', tr('contrib.settings.noReceipts')));
    for (const c of list) {
      const row = el('div', 'cedar-set-integration');
      const main = el('div', 'cedar-set-integration-main');
      const when = new Date(c.submitted_at).toLocaleDateString(cedarI18n.locale);
      main.append(el('strong', '', c.title || tr('contrib.settings.untitled')), el('small', '', `${when} · ${c.license} · ${c.id}`));
      const side = el('div', 'cedar-set-integration-side');
      if (c.withdrawn_at) side.append(el('span', 'cedar-set-badge', tr('contrib.settings.withdrawn')));
      else side.append(btn(tr('contrib.settings.withdraw'), async () => {
        if (!(await cedarConfirm(tr('contrib.settings.withdrawConfirm'), { ok: tr('contrib.settings.withdraw') }))) return;
        try {
          await deps.api('DELETE', `/contributions/${encodeURIComponent(c.id)}`);
          receipts = await deps.api('GET', '/contributions');
        } catch (err) {
          deps.toast(tr('contrib.settings.saveFailed', { error: err.message }));
        }
        drawSettings(section);
        shell()?.refresh('contribute');
      }, 'settings-ghost'));
      row.append(main, side);
      rec.append(row);
    }
    parts.push(rec);
    section.replaceChildren(...parts);
  }

  async function renderSettings(section) {
    if (!settingsData) section.replaceChildren(el('p', 'cedar-set-note', tr('contrib.settings.loading')));
    try {
      [settingsData, receipts] = await Promise.all([deps.api('GET', '/settings/contributions'), deps.api('GET', '/contributions')]);
    } catch (err) {
      section.replaceChildren(el('p', 'cedar-set-error', tr('contrib.settings.loadFailed', { error: err.message })));
      return;
    }
    drawSettings(section);
  }

  function settingsSummary() {
    if (!settingsData) return null;
    if (!settingsData.available) return tr('contrib.settings.summaryUnavailable');
    if (!settingsData.accepted_at) return tr('contrib.settings.summaryOff');
    const sent = (receipts?.contributions || []).filter((c) => !c.withdrawn_at).length;
    return { text: tr('contrib.settings.summaryOn', { count: sent }), kind: 'ok' };
  }

  function bindSettings() {
    const s = shell();
    if (!s) return;
    const body = document.getElementById('settings-body');
    if (!body) return;
    let section = body.querySelector('.settings-page[data-page="contribute"]');
    if (!section) {
      section = el('section', 'settings-page');
      section.dataset.page = 'contribute';
      section.id = 'contribute-section';
      section.hidden = true;
      body.append(section);
    }
    s.register({
      id: 'contribute',
      label: tr('contrib.settings.title'),
      title: tr('contrib.settings.title'),
      description: tr('contrib.settings.description'),
      render: renderSettings,
      summary: settingsSummary,
      footer: false,
    });
  }

  function init(d) {
    deps = d;
    bindMenu();
    bindSettings();
  }

  return { init, open, refreshMenu };
})();

'use strict';

// Handing work over from the agent to the person, shared by every UI. Three things, all on the
// rule the follow-up card set: the model proposes, a card shows it, and nothing happens until a
// button is pressed.
//
//   - What Cedar could not do (cedar's `report_limitation` tool). The tool records nothing; its
//     result is {"limitation": {"summary", "category", "needed"?}, "note"}, drawn as a card with
//     Send feedback and Dismiss (PUT /api/turns/{turn}/limitations/{call}). Sent feedback stays
//     on this server, listed under Settings -> Feedback, which this file also draws.
//   - What waits for approval. A call that would send or change something in the person's
//     accounts, made by a run nobody was answering, is not made: its result is {"held": {"id",
//     "tool", "reason", "why"}, "note"}. The card shows what would go out and offers Approve,
//     Edit and Discard (POST /api/outbox/{id}); approving makes exactly that call, or the edited
//     one, with no model involved.
//   - The outbox: every call still waiting, wherever its run was, for the Agents page.
//
//   cedarHandover.known(limitations)                        the decisions a conversation has
//   cedarHandover.limitation.parse(output) -> spec | null
//   cedarHandover.limitation.element(spec, { turnId, callId })
//   cedarHandover.held.parse(output) -> spec | null
//   cedarHandover.held.element(spec)
//   cedarHandover.outbox.section() -> element               hidden while nothing waits
//
// Everything in a result came from the model, so text is only ever set as text. The module
// talks to /api itself, like the Records page, so a UI only has to place the cards.

const cedarHandover = (() => {
  const ADMIN_KEY = 'cedar-chat-admin';
  const CATEGORIES = ['missing_tool', 'not_connected', 'blocked', 'limit', 'other'];
  const decided = new Map(); // "turn/call" -> feedback

  function el(tag, className, text) {
    const e = document.createElement(tag);
    if (className) e.className = className;
    if (text !== undefined) e.textContent = text;
    return e;
  }

  function button(label, className, onClick) {
    const b = el('button', className, label);
    b.type = 'button';
    b.addEventListener('click', onClick);
    return b;
  }

  const str = (v, max) => (typeof v === 'string' && v.trim() ? v.trim().slice(0, max) : '');

  async function api(method, path, body, { admin = false } = {}) {
    const headers = {};
    try {
      const token = localStorage.getItem('cedar-chat-token');
      if (token) headers.Authorization = `Bearer ${token}`;
      if (admin) headers['X-Admin-Token'] = sessionStorage.getItem(ADMIN_KEY) || '';
    } catch { /* no saved token: the server may not need one */ }
    if (body !== undefined) headers['Content-Type'] = 'application/json';
    const res = await fetch(`/api${path}`, { method, headers, body: body === undefined ? undefined : JSON.stringify(body) });
    if (res.status === 204) return null;
    const data = await res.json().catch(() => null);
    if (!res.ok) {
      const error = new Error(data?.error?.message || tr('handover.requestFailed', { status: String(res.status) }));
      error.status = res.status;
      throw error;
    }
    return data;
  }

  // Where a report can be filed as an issue (the server's CHAT_FEEDBACK_ISSUES_URL), asked once.
  // /api/config is public, so this needs no token.
  let issuesUrl;
  function issuesTarget() {
    issuesUrl ||= fetch('/api/config').then((r) => (r.ok ? r.json() : null)).then((c) => {
      const url = typeof c?.feedback_issues_url === 'string' ? c.feedback_issues_url : '';
      return url.startsWith('https://') ? url : '';
    }).catch(() => '');
    return issuesUrl;
  }

  /**
   * A link that opens a new issue with the report filled in, for the person to read and submit
   * themselves, under their own account: this page sends nothing. The issue is in English, as
   * its readers are the project's maintainers, and holds only what the card shows. Resolves to
   * null when the server offers no such address.
   */
  async function issueLink(report) {
    const base = await issuesTarget();
    if (!base || !report?.summary) return null;
    const lines = ['**What Cedar could not do**', report.summary, '', `**Category:** ${CATEGORIES.includes(report.category) ? report.category : 'other'}`];
    if (report.needed) lines.push(`**Would need:** ${report.needed}`);
    if (report.comment) lines.push('', '**Note**', report.comment);
    lines.push('', '_Filed from the feedback card in Cedar\'s chat. It holds only what the card showed, nothing else of the conversation._');
    const query = new URLSearchParams({ title: `Feedback: ${report.summary}`.slice(0, 200), body: lines.join('\n'), labels: 'feedback' });
    const a = el('a', 'copy-btn cedar-handover-issue', tr('handover.issue.open'));
    a.href = `${base}?${query}`;
    a.target = '_blank';
    a.rel = 'noopener noreferrer';
    a.title = tr('handover.issue.note');
    return a;
  }

  const time = (iso) => new Date(iso || Date.now()).toLocaleString(cedarI18n.locale, { day: 'numeric', month: 'short', hour: '2-digit', minute: '2-digit' });

  // -- what Cedar could not do -------------------------------------------------------------------

  /** The report in a `report_limitation` result, or null if `output` isn't one. */
  function parseLimitation(output) {
    let data;
    try { data = JSON.parse(output); } catch { return null; }
    const l = data?.limitation;
    if (!l || typeof l !== 'object') return null;
    const summary = str(l.summary, 300);
    if (!summary) return null;
    return { summary, category: CATEGORIES.includes(l.category) ? l.category : 'other', needed: str(l.needed, 200) };
  }

  /** Take in what was already decided in the conversation being opened. */
  function known(limitations) {
    for (const f of limitations || []) if (f.turn_id) decided.set(`${f.turn_id}/${f.call_id}`, f);
  }

  /**
   * "Show Cedar how": the other answer to something it couldn't do. It opens the screen recorder
   * of the Actions page (actions.js) with the report as what is about to be shown; the steps a
   * model reads from the recording are saved as an action that later runs follow. Null where
   * the page isn't loaded. Reporting and showing are apart: this decides nothing about the card.
   */
  function showHow(spec) {
    if (typeof cedarActions === 'undefined') return null;
    const b = button(tr('actions.showHow'), 'copy-btn cedar-handover-show', () => cedarActions.record(spec.summary));
    b.title = tr('actions.showHowNote');
    return b;
  }

  function limitationElement(spec, { turnId, callId }) {
    const key = `${turnId}/${callId}`;
    const card = el('div', 'cedar-handover cedar-limitation');
    const head = el('div', 'cedar-handover-head');
    head.append(el('span', 'cedar-handover-label', tr('handover.limitation.label')), el('span', 'cedar-handover-chip', tr(`handover.category.${spec.category}`)));
    card.append(head, el('div', 'cedar-handover-body', spec.summary));
    if (spec.needed) card.append(el('div', 'cedar-handover-sub', tr('handover.limitation.needed', { needed: spec.needed })));
    const comment = el('input', 'cedar-handover-input');
    comment.type = 'text';
    comment.maxLength = 1000;
    comment.placeholder = tr('handover.limitation.comment');
    comment.setAttribute('aria-label', tr('handover.limitation.comment'));
    const foot = el('div', 'cedar-handover-foot');
    card.append(comment, foot);

    const draw = (failed) => {
      const done = decided.get(key);
      foot.textContent = '';
      comment.hidden = !!done;
      card.classList.toggle('settled', !!done);
      if (done) {
        foot.append(el('span', done.status === 'sent' ? 'cedar-handover-done' : 'cedar-handover-note', tr(done.status === 'sent' ? 'handover.limitation.sent' : 'handover.limitation.dismissed')));
        // What stays on this server reaches whoever runs it; the project hears of it only if
        // the person files it there themselves.
        if (done.status === 'sent') issueLink({ ...spec, comment: done.comment }).then((a) => { if (a && decided.get(key) === done) foot.append(a); });
        const show = showHow(spec);
        if (show) foot.append(show);
        return;
      }
      const act = (label, cls, body) => button(label, cls, async () => {
        for (const b of foot.querySelectorAll('button')) b.disabled = true;
        let error = '';
        try {
          const { feedback } = await api('PUT', `/turns/${turnId}/limitations/${encodeURIComponent(callId)}`, body());
          decided.set(key, feedback);
        } catch (e) { error = e.message || tr('handover.failed'); }
        draw(error);
      });
      foot.append(el('span', failed ? 'cedar-handover-error' : 'cedar-handover-note', failed || tr('handover.limitation.note')));
      foot.append(
        act(tr('handover.limitation.send'), 'primary', () => ({ decision: 'send', summary: spec.summary, category: spec.category, needed: spec.needed || undefined, comment: comment.value.trim() || undefined })),
        act(tr('handover.dismiss'), 'copy-btn', () => ({ decision: 'dismiss' })),
      );
      const show = showHow(spec);
      if (show) foot.append(show);
    };
    draw();
    return card;
  }

  // -- what waits for approval ---------------------------------------------------------------------

  /** The held call in a tool result, or null if `output` isn't one. */
  function parseHeld(output) {
    let data;
    try { data = JSON.parse(output); } catch { return null; }
    const h = data?.held;
    if (!h || typeof h !== 'object') return null;
    const id = str(h.id, 64);
    if (!/^[0-9a-f-]{36}$/i.test(id)) return null;
    return { id, tool: str(h.tool, 200), reason: str(h.reason, 600), why: h.why === 'timeout' ? 'timeout' : 'unattended' };
  }

  /** A held call's input as rows of name and value: text as it is, anything else as JSON. */
  function inputRows(input, edit) {
    const box = el('div', 'cedar-handover-fields');
    const fields = new Map();
    for (const [name, value] of Object.entries(input && typeof input === 'object' ? input : {})) {
      const row = el('label', 'cedar-handover-field');
      row.append(el('span', 'cedar-handover-field-name', name));
      if (edit && typeof value === 'string') {
        const area = el('textarea', 'cedar-handover-input');
        area.value = value;
        area.rows = Math.min(10, Math.max(1, value.split('\n').length + (value.length > 80 ? 1 : 0)));
        fields.set(name, area);
        row.append(area);
      } else {
        row.append(el('span', 'cedar-handover-field-value', typeof value === 'string' ? value : JSON.stringify(value)));
      }
      box.append(row);
    }
    return { box, fields };
  }

  /**
   * The card for a held call. `spec` is what the tool result said ({id, tool, reason, why}); what
   * became of it since is asked of the server, or given as `item` by a list that already has it.
   * `onChange(item)` hears each decision.
   */
  function heldElement(spec, { item = null, onChange } = {}) {
    const card = el('div', 'cedar-handover cedar-held');
    const head = el('div', 'cedar-handover-head');
    const label = el('span', 'cedar-handover-label', tr('handover.held.label'));
    head.append(label, el('span', 'cedar-handover-chip', spec.tool));
    const body = el('div', 'cedar-handover-body', spec.reason || spec.tool);
    const detail = el('div', 'cedar-handover-detail');
    const foot = el('div', 'cedar-handover-foot');
    card.append(head, body, detail, foot);
    let editing = false;

    const draw = (failed) => {
      detail.textContent = '';
      foot.textContent = '';
      card.dataset.state = item?.status || 'loading';
      const settled = !!item && !['pending', 'failed'].includes(item.status);
      card.classList.toggle('settled', settled);
      label.textContent = tr(settled ? 'handover.held.labelDone' : 'handover.held.label');
      if (!item) {
        foot.append(el('span', failed ? 'cedar-handover-error' : 'cedar-handover-note', failed || tr('handover.loading')));
        return;
      }
      if (item.status === 'gone') {
        foot.append(el('span', 'cedar-handover-note', tr('handover.held.gone')));
        return;
      }
      const open = item.status === 'pending' || item.status === 'failed';
      const { box, fields } = inputRows(item.input, editing && open);
      if (editing && open) {
        detail.append(box);
      } else {
        const what = el('details', 'cedar-handover-what');
        what.open = open;
        what.append(el('summary', '', tr(open ? 'handover.held.willSend' : 'handover.held.wasSent')), box);
        detail.append(what);
      }
      if (item.status === 'failed' && item.output) detail.append(el('div', 'cedar-handover-error', tr('handover.held.failed', { error: item.output })));
      if (!open) {
        const key = { sent: item.edited ? 'handover.held.sentEdited' : 'handover.held.sent', discarded: 'handover.held.discarded', sending: 'handover.held.sending' }[item.status];
        foot.append(el('span', item.status === 'sent' ? 'cedar-handover-done' : 'cedar-handover-note', key ? tr(key, { when: time(item.decided_at) }) : item.status));
        return;
      }
      const decide = (label, cls, body) => button(label, cls, async () => {
        for (const b of foot.querySelectorAll('button')) b.disabled = true;
        let error = '';
        try {
          item = await api('POST', `/outbox/${spec.id}`, body());
          editing = false;
          onChange?.(item);
        } catch (e) {
          error = e.message || tr('handover.failed');
          // Decided elsewhere meanwhile: show what became of it.
          if (e.status === 409) { try { item = await api('GET', `/outbox/${spec.id}`); editing = false; } catch { /* keep the error */ } }
        }
        draw(error);
      });
      const edited = () => {
        const input = { ...item.input };
        for (const [name, area] of fields) input[name] = area.value;
        return input;
      };
      foot.append(el('span', failed ? 'cedar-handover-error' : 'cedar-handover-note', failed || tr(spec.why === 'timeout' ? 'handover.held.noteTimeout' : 'handover.held.note')));
      if (editing) {
        foot.append(
          decide(tr('handover.held.approveEdited'), 'primary', () => ({ decision: 'approve', input: edited() })),
          button(tr('handover.cancel'), 'copy-btn', () => { editing = false; draw(); }),
        );
      } else {
        foot.append(decide(tr(item.status === 'failed' ? 'handover.held.retry' : 'handover.held.approve'), 'primary', () => ({ decision: 'approve' })));
        if (Object.values(item.input || {}).some((v) => typeof v === 'string')) foot.append(button(tr('handover.held.edit'), 'copy-btn', () => { editing = true; draw(); }));
        foot.append(decide(tr('handover.held.discard'), 'copy-btn', () => ({ decision: 'discard' })));
      }
    };
    draw();
    if (!item) {
      api('GET', `/outbox/${spec.id}`).then(
        (found) => { item = found; draw(); },
        (e) => { if (e.status === 404) { item = { status: 'gone' }; draw(); } else draw(e.message || tr('handover.failed')); },
      );
    }
    return card;
  }

  /**
   * Everything that waits for approval, for the Agents page: a section that is hidden while
   * nothing waits and fills itself. Each call says which agent's run (or which conversation)
   * wanted to make it.
   */
  function outboxSection() {
    const section = el('section', 'cedar-outbox');
    section.hidden = true;
    const list = el('div', 'cedar-outbox-list');
    section.append(el('h2', 'cedar-outbox-title', tr('handover.outbox.title')), el('p', 'cedar-outbox-intro', tr('handover.outbox.intro')), list);
    const load = async () => {
      let items;
      try { ({ items } = await api('GET', '/outbox')); } catch { return; }
      list.textContent = '';
      for (const item of items || []) {
        const row = el('div', 'cedar-outbox-item');
        const from = el('div', 'cedar-outbox-from');
        const name = item.agent?.title || item.title;
        if (item.conversation_id) {
          const a = el('a', '', name || tr('handover.outbox.open'));
          a.href = `#/c/${item.conversation_id}`;
          from.append(a);
        } else if (name) {
          from.append(el('span', '', name));
        }
        from.append(el('span', 'cedar-handover-note', time(item.created_at)));
        row.append(from, heldElement({ id: item.id, tool: item.tool, reason: item.reason, why: item.why }, { item }));
        list.append(row);
      }
      section.hidden = !(items || []).length;
    };
    load();
    return section;
  }

  // -- Settings -> Feedback ------------------------------------------------------------------------

  const shell = () => (typeof cedarSettingsShell === 'undefined' ? null : cedarSettingsShell);
  let feedback = null; // the last GET /api/feedback
  let everyone = false; // whether it is all users' (the admin token is there)

  async function loadFeedback() {
    let admin = '';
    try { admin = sessionStorage.getItem(ADMIN_KEY) || ''; } catch { /* then only one's own */ }
    if (admin) {
      try {
        feedback = await api('GET', '/feedback?users=all', undefined, { admin: true });
        everyone = true;
        return;
      } catch { /* a wrong or expired admin token: one's own instead */ }
    }
    feedback = await api('GET', '/feedback');
    everyone = false;
  }

  async function renderSettings(section) {
    section.textContent = '';
    try { await loadFeedback(); } catch (e) {
      section.append(el('p', 'cedar-set-note', e.message || tr('handover.failed')));
      return;
    }
    const items = feedback.feedback || [];
    if (!items.length) {
      section.append(el('p', 'cedar-set-note', tr('handover.feedback.empty')));
      return;
    }
    section.append(el('p', 'cedar-set-note', tr(everyone ? 'handover.feedback.introAll' : 'handover.feedback.intro')));
    const counts = el('div', 'cedar-handover-counts');
    for (const c of feedback.by_category || []) counts.append(el('span', 'cedar-handover-chip', `${tr(`handover.category.${c.category}`)} · ${c.count}`));
    section.append(counts);
    const list = el('div', 'cedar-feedback-list');
    for (const f of items) {
      const row = el('div', 'cedar-feedback-row');
      const main = el('div', 'cedar-feedback-main');
      main.append(el('div', 'cedar-feedback-summary', f.summary || ''));
      if (f.needed) main.append(el('div', 'cedar-set-note', tr('handover.limitation.needed', { needed: f.needed })));
      if (f.comment) main.append(el('div', 'cedar-feedback-comment', f.comment));
      const meta = [tr(`handover.category.${CATEGORIES.includes(f.category) ? f.category : 'other'}`), time(f.created_at)];
      if (f.user_name) meta.unshift(f.user_name);
      main.append(el('div', 'cedar-set-note', meta.join(' · ')));
      const acts = el('div', 'cedar-feedback-acts');
      issueLink(f).then((a) => { if (a) acts.prepend(a); });
      row.append(main, acts);
      acts.append(button(tr('handover.feedback.delete'), 'copy-btn', async (e) => {
        e.currentTarget.disabled = true;
        try {
          await api('DELETE', `/feedback/${f.id}${everyone ? '?users=all' : ''}`, undefined, { admin: everyone });
          await renderSettings(section);
          shell()?.refresh('feedback');
        } catch { e.currentTarget.disabled = false; }
      }));
      list.append(row);
    }
    section.append(list);
  }

  function summary() {
    if (!feedback) return null;
    const count = (feedback.feedback || []).length;
    return count ? tr('handover.feedback.count', { count }) : tr('handover.feedback.none');
  }

  function bind() {
    if (!shell()) return;
    const body = document.getElementById('settings-body');
    if (!body) return;
    let section = body.querySelector('.settings-page[data-page="feedback"]');
    if (!section) {
      section = el('section', 'settings-page');
      section.dataset.page = 'feedback';
      section.id = 'feedback-section';
      section.hidden = true;
      body.append(section);
    }
    shell().register({
      id: 'feedback',
      label: tr('handover.feedback.title'),
      title: tr('handover.feedback.title'),
      description: tr('handover.feedback.description'),
      render: renderSettings,
      summary,
      footer: false,
    });
  }

  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', bind);
  else bind();

  return {
    known,
    limitation: { parse: parseLimitation, element: limitationElement },
    held: { parse: parseHeld, element: heldElement },
    outbox: { section: outboxSection },
  };
})();

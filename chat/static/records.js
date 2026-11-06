'use strict';

// Settings → Records: the data Cedar keeps for the user across conversations (cedar's
// `/v1/records`, through `/api/records`). Each set is listed with its description, size, columns,
// key and when it was last saved; Show rows loads the set's records into a table, newest first,
// and Delete removes the set after a confirmation. Delete all clears every set. What is listed is
// exactly what the agent is told about, and a deletion applies from the next message on, so there
// is nothing to save. The page draws its own section, like the Memory page, and needs only its
// script tag after settings-shell.js.

(() => {
  const SHOW_ROWS = 50; // how many of a set's records the table shows at first

  let data = null;      // the last GET /api/records: { supported, sets, limits }
  let tools = undefined; // the saved default tool selection (null: every group)
  const open = new Map(); // set name -> { records, shown } while its rows are showing

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

  const shell = () => (typeof cedarSettingsShell === 'undefined' ? null : cedarSettingsShell);

  async function api(method, path) {
    const headers = {};
    try {
      const token = localStorage.getItem('cedar-chat-token');
      if (token) headers.Authorization = `Bearer ${token}`;
    } catch { /* no saved token: the server may not need one */ }
    const res = await fetch(`/api${path}`, { method, headers });
    if (res.status === 204) return null;
    const body = await res.json().catch(() => null);
    if (!res.ok) throw new Error(body?.error?.message || tr('records.requestFailed', { status: String(res.status) }));
    return body;
  }

  function when(iso) {
    if (!iso) return '';
    return new Date(iso).toLocaleDateString(cedarI18n.locale, { day: 'numeric', month: 'short', year: 'numeric' });
  }

  /** Whether the saved tool selection leaves the records group out, so new conversations don't use it. */
  function offInTools() {
    return Array.isArray(tools) && !tools.includes('*') && !tools.includes('records');
  }

  function closeSettings() {
    document.getElementById('settings-dialog')?.close();
  }

  /** A cell's text: strings as they are, anything else as JSON. */
  function cell(value) {
    if (value === null || value === undefined) return '';
    return typeof value === 'string' ? value : JSON.stringify(value);
  }

  /** Where a record came from: a link to its conversation, or nothing. */
  function source(record) {
    if (record.conversation_id) {
      const a = el('a', 'settings-link', record.title || tr('records.untitled'));
      a.href = `#/c/${record.conversation_id}`;
      a.addEventListener('click', closeSettings);
      return a;
    }
    return el('span', '', record.run_id ? tr('records.fromElsewhere') : tr('records.noSource'));
  }

  /** The table of a set's rows, newest first, with the columns the set has. */
  function table(set, state) {
    const wrap = el('div', 'cedar-rec-rows');
    const t = el('table', 'cedar-rec-table');
    const columns = set.columns?.length ? set.columns : [...new Set(state.records.flatMap((r) => Object.keys(r.data || {})))];
    const head = el('tr');
    for (const c of columns) head.append(el('th', '', c));
    head.append(el('th', '', tr('records.savedAt')), el('th', '', tr('records.source')));
    const thead = el('thead');
    thead.append(head);
    const tbody = el('tbody');
    const newest = [...state.records].reverse().slice(0, state.shown);
    for (const r of newest) {
      const row = el('tr');
      for (const c of columns) {
        const td = el('td', '', cell(r.data?.[c]));
        td.title = td.textContent;
        row.append(td);
      }
      row.append(el('td', '', when(r.updated_at)));
      const src = el('td');
      src.append(source(r));
      row.append(src);
      tbody.append(row);
    }
    t.append(thead, tbody);
    wrap.append(t);
    return wrap;
  }

  async function toggleRows(set, section) {
    if (open.has(set.name)) {
      open.delete(set.name);
      draw(section);
      return;
    }
    try {
      const body = await api('GET', `/records/${encodeURIComponent(set.name)}`);
      open.set(set.name, { records: body.records || [], shown: SHOW_ROWS });
    } catch (err) {
      shell()?.setStatus('records', err.message, 'error');
    }
    draw(section);
  }

  async function dropSet(set, section) {
    if (!(await cedarConfirm(tr('records.confirmDelete', { name: set.name, count: set.count }), { ok: tr('records.delete') }))) return;
    try {
      await api('DELETE', `/records/${encodeURIComponent(set.name)}`);
      data.sets = data.sets.filter((s) => s.name !== set.name);
      open.delete(set.name);
    } catch (err) {
      shell()?.setStatus('records', err.message, 'error');
    }
    draw(section);
  }

  async function dropAll(section) {
    const count = data?.sets?.length || 0;
    if (!count) return;
    if (!(await cedarConfirm(tr('records.confirmDeleteAll', { count }), { ok: tr('records.deleteAll') }))) return;
    try {
      await api('DELETE', '/records');
      data.sets = [];
      open.clear();
    } catch (err) {
      shell()?.setStatus('records', err.message, 'error');
    }
    draw(section);
  }

  function row(set, section) {
    const r = el('div', 'cedar-set-integration');
    const main = el('div', 'cedar-set-integration-main');
    const title = el('div', 'cedar-rec-meta');
    title.append(el('strong', '', set.name));
    if (set.health_data) title.append(el('span', 'cedar-set-badge', tr('records.health')));
    main.append(title);
    if (set.description) main.append(el('span', '', set.description));
    const sub = el('small', '');
    const key = set.key_fields?.length ? tr('records.key', { key: set.key_fields.join(', ') }) : tr('records.noKey');
    sub.append(`${tr('records.rows', { count: set.count })} · ${key} · ${tr('records.lastSaved', { date: when(set.updated_at) })}`);
    main.append(sub);
    if (set.columns?.length) main.append(el('small', 'cedar-rec-cols', set.columns.join(', ')));
    const side = el('div', 'cedar-set-integration-side');
    const state = open.get(set.name);
    if (set.count > 0) side.append(button(state ? tr('records.hideRows') : tr('records.showRows'), () => toggleRows(set, section), 'settings-ghost'));
    side.append(button(tr('records.delete'), () => dropSet(set, section)));
    r.append(main, side);
    if (state) {
      r.append(table(set, state));
      if (state.records.length > state.shown) {
        const more = el('div', 'cedar-rec-meta');
        more.append(
          el('small', '', tr('records.showingOf', { shown: state.shown, count: state.records.length })),
          button(tr('records.showMore'), () => { state.shown += SHOW_ROWS * 4; draw(section); }, 'settings-link'),
        );
        r.append(more);
      }
    }
    return r;
  }

  function draw(section) {
    shell()?.refresh('records');
    if (!data) return;
    if (!data.supported) {
      section.replaceChildren(el('p', 'cedar-set-note', tr('records.unsupported')));
      return;
    }
    const parts = [];
    if (offInTools()) {
      const warn = el('p', 'cedar-set-warn', tr('records.offInTools'));
      warn.append(' ', button(tr('records.openTools'), () => shell()?.go('tools'), 'settings-link'));
      parts.push(warn);
    }
    const block = el('div', 'cedar-set-block');
    const head = el('div', 'cedar-set-block-head');
    const max = data.limits?.max_sets;
    head.append(el('span', 'settings-eyebrow', max
      ? tr('records.countOf', { count: data.sets.length, max })
      : tr('records.count', { count: data.sets.length })));
    if (data.sets.length) head.append(button(tr('records.deleteAll'), () => dropAll(section), 'settings-ghost'));
    block.append(head);
    if (data.sets.length) {
      const list = el('div', 'cedar-set-models');
      // Most recently saved first.
      const sets = [...data.sets].sort((a, b) => String(b.updated_at).localeCompare(String(a.updated_at)));
      for (const set of sets) list.append(row(set, section));
      block.append(list);
    } else {
      block.append(el('p', 'cedar-set-note', tr('records.empty')));
    }
    parts.push(block, el('p', 'cedar-set-note', tr('records.privacy')));
    section.replaceChildren(...parts);
  }

  async function render(section) {
    shell()?.setStatus('records', '');
    if (!data) section.replaceChildren(el('p', 'cedar-set-note', tr('records.loading')));
    try {
      const [records, prefs] = await Promise.all([api('GET', '/records'), api('GET', '/settings').catch(() => null)]);
      data = records;
      tools = prefs ? prefs.tools : undefined;
    } catch (err) {
      section.replaceChildren(el('p', 'cedar-set-error', tr('records.loadFailed', { error: err.message })));
      return;
    }
    draw(section);
  }

  function summary() {
    if (!data) return null;
    if (!data.supported) return tr('records.summaryUnsupported');
    if (offInTools()) return { text: tr('records.summaryOff'), kind: 'warn' };
    return tr('records.summary', { count: data.sets.length });
  }

  function bind() {
    if (!shell()) return;
    const body = document.getElementById('settings-body');
    if (!body) return;
    let section = body.querySelector('.settings-page[data-page="records"]');
    if (!section) {
      section = el('section', 'settings-page');
      section.dataset.page = 'records';
      section.id = 'records-section';
      section.hidden = true;
      body.append(section);
    }
    shell().register({
      id: 'records',
      label: tr('records.title'),
      title: tr('records.title'),
      description: tr('records.description'),
      render,
      summary,
      footer: false,
    });
  }

  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', bind);
  else bind();
})();

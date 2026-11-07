'use strict';

// Settings → Memory: what Cedar remembers about the user across conversations (cedar's
// `/v1/memory`, through `/api/memory`). At the top, the profile: the short summary of the user
// that Cedar reads before every conversation, written by its background pass unless the user
// writes it themselves. Below, the memories by kind: facts, preferences and standing
// instructions the user stated (each with the words it was taken from and the conversation it
// came from), what past conversations were about, and ways of working that went well; and the
// archive, where superseded, expired and forgotten memories stay until deleted. Each row can be
// pinned (always shown to Cedar), archived or restored, and removed for good. The page draws its
// own section, like the Language page, and needs only its script tag after settings-shell.js.

(() => {
  let data = null;      // the last GET /api/memory: { supported, items, profile, limits }
  let total = null;     // how many active memories there are of every kind, for the menu summary
  let tools = undefined; // the saved default tool selection (null: every group)
  let view = 'about';    // 'about' | 'episodes' | 'procedures' | 'archived'

  const VIEWS = {
    about: { kinds: ['fact', 'preference', 'instruction', 'temporary'], archived: false },
    episodes: { kinds: ['episode'], archived: false },
    procedures: { kinds: ['procedure'], archived: false },
    archived: { kinds: [], archived: true },
  };
  // Every active kind: the menu counts all memories, not just the ones the open view lists.
  const ALL_KINDS = [...new Set(Object.values(VIEWS).flatMap((v) => v.kinds))];

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

  async function api(method, path, body) {
    const headers = {};
    try {
      const token = localStorage.getItem('cedar-chat-token');
      if (token) headers.Authorization = `Bearer ${token}`;
    } catch { /* no saved token: the server may not need one */ }
    if (body !== undefined) headers['Content-Type'] = 'application/json';
    const res = await fetch(`/api${path}`, { method, headers, body: body === undefined ? undefined : JSON.stringify(body) });
    if (res.status === 204) return null;
    const json = await res.json().catch(() => null);
    if (!res.ok) throw new Error(json?.error?.message || tr('memory.requestFailed', { status: String(res.status) }));
    return json;
  }

  function when(iso) {
    if (!iso) return '';
    return new Date(iso).toLocaleDateString(cedarI18n.locale, { day: 'numeric', month: 'short', year: 'numeric' });
  }

  /** Whether the saved tool selection leaves the memory group out, so new conversations don't use it. */
  function offInTools() {
    return Array.isArray(tools) && !tools.includes('*') && !tools.includes('memory');
  }

  function closeSettings() {
    document.getElementById('settings-dialog')?.close();
  }

  /** Where a memory came from: a link to its conversation, or why there is none. */
  function source(item) {
    if (item.conversation_id) {
      const a = el('a', 'settings-link', item.title || tr('memory.untitled'));
      a.href = `#/c/${item.conversation_id}`;
      a.addEventListener('click', closeSettings);
      return a;
    }
    if (item.source === 'consolidation') return el('span', '', tr('memory.fromConsolidation'));
    // No run: added by hand, or its run was deleted with its conversation.
    return el('span', '', item.run_id ? tr('memory.fromElsewhere') : tr('memory.noSource'));
  }

  function fail(err) {
    shell()?.setStatus('memory', err.message, 'error');
  }

  async function change(item, body, section) {
    try {
      const updated = await api('PATCH', `/memory/${encodeURIComponent(item.id)}`, body);
      Object.assign(item, updated);
    } catch (err) {
      fail(err);
    }
    await reload(section);
  }

  function row(item, section) {
    const r = el('div', 'cedar-set-integration');
    const main = el('div', 'cedar-set-integration-main');
    const head = el('span', '');
    head.append(el('span', 'cedar-set-badge', tr(`memory.kind.${item.kind}`)), ' ');
    if (item.pinned) head.append(el('span', 'cedar-set-badge', tr('memory.pinned')), ' ');
    head.append(item.text);
    main.append(head);
    if (item.support && item.support !== item.text) {
      main.append(el('small', '', tr('memory.said', { quote: item.support })));
    }
    if (item.kind === 'procedure' && item.meta?.pitfall) main.append(el('small', '', tr('memory.pitfall', { text: item.meta.pitfall })));
    if (item.kind === 'episode' && item.meta?.outcome && item.meta.outcome !== 'completed') {
      main.append(el('small', '', tr('memory.outcome', { outcome: item.meta.outcome })));
    }
    const sub = el('small', '');
    const bits = [when(item.valid_from || item.created_at)];
    if (item.valid_until) bits.push(tr('memory.until', { date: when(item.valid_until) }));
    if (item.superseded_by) bits.push(tr('memory.superseded'));
    else if (item.archived_at) bits.push(tr('memory.archivedOn', { date: when(item.archived_at) }));
    if (item.recall_count) bits.push(tr('memory.used', { count: item.recall_count }));
    sub.append(`${bits.join(' · ')} · `, source(item));
    main.append(sub);
    const side = el('div', 'cedar-set-integration-side');
    const aboutUser = !['episode', 'procedure'].includes(item.kind);
    const active = !item.archived_at && !item.superseded_by;
    if (active && aboutUser) {
      side.append(button(item.pinned ? tr('memory.unpin') : tr('memory.pin'), () => change(item, { pinned: !item.pinned }, section)));
    }
    if (active) side.append(button(tr('memory.archive'), () => change(item, { archived: true }, section)));
    else if (!item.superseded_by) side.append(button(tr('memory.restore'), () => change(item, { archived: false }, section)));
    side.append(button(tr('memory.remove'), async (e) => {
      e.currentTarget.disabled = true;
      try {
        await api('DELETE', `/memory/${encodeURIComponent(item.id)}`);
      } catch (err) {
        fail(err);
      }
      await reload(section);
    }));
    r.append(main, side);
    return r;
  }

  async function forgetAll(section) {
    const count = data?.items?.length || 0;
    if (!count && !data?.profile) return;
    if (!(await cedarConfirm(tr('memory.confirmForgetAll', { count }), { ok: tr('memory.forgetAll') }))) return;
    try {
      await api('DELETE', '/memory');
    } catch (err) {
      fail(err);
    }
    await reload(section);
  }

  async function exportAll() {
    try {
      const all = await api('GET', '/memory/export');
      const blob = new Blob([JSON.stringify(all, null, 2)], { type: 'application/json' });
      const a = document.createElement('a');
      a.href = URL.createObjectURL(blob);
      a.download = `cedar-memory-${new Date().toISOString().slice(0, 10)}.json`;
      a.click();
      setTimeout(() => URL.revokeObjectURL(a.href), 1000);
    } catch (err) {
      fail(err);
    }
  }

  function profileBlock(section) {
    const block = el('div', 'cedar-set-block');
    const head = el('div', 'cedar-set-block-head');
    head.append(el('span', 'settings-eyebrow', tr('memory.profile')));
    block.append(head);
    const profile = data.profile;
    const area = el('textarea', 'settings-input');
    area.rows = 4;
    area.placeholder = tr('memory.profilePlaceholder');
    area.value = profile?.text || '';
    area.maxLength = data.limits?.max_profile_chars || 1600;
    const note = el('p', 'cedar-set-note', profile
      ? (profile.user_edited ? tr('memory.profileByYou', { date: when(profile.updated_at) }) : tr('memory.profileByCedar', { date: when(profile.updated_at) }))
      : tr('memory.profileNone'));
    const actions = el('div', 'cedar-set-block-head');
    const save = button(tr('memory.saveProfile'), async () => {
      save.disabled = true;
      try {
        await api('PUT', '/memory/profile', { text: area.value });
      } catch (err) {
        fail(err);
      }
      await reload(section);
    }, 'settings-ghost');
    save.disabled = true;
    area.addEventListener('input', () => { save.disabled = area.value === (profile?.text || ''); });
    actions.append(note, save);
    block.append(area, actions);
    return block;
  }

  function draw(section) {
    shell()?.refresh('memory');
    if (!data) return;
    if (!data.supported) {
      section.replaceChildren(el('p', 'cedar-set-note', tr('memory.unsupported')));
      return;
    }
    const parts = [];
    if (offInTools()) {
      const warn = el('p', 'cedar-set-warn', tr('memory.offInTools'));
      warn.append(' ', button(tr('memory.openTools'), () => shell()?.go('tools'), 'settings-link'));
      parts.push(warn);
    }
    parts.push(profileBlock(section));

    const block = el('div', 'cedar-set-block');
    const tabs = el('div', 'cedar-set-block-head');
    const chips = el('div', '');
    for (const id of Object.keys(VIEWS)) {
      const chip = button(tr(`memory.view.${id}`), () => { view = id; reload(section); }, 'settings-ghost');
      if (id === view) chip.setAttribute('aria-pressed', 'true');
      chips.append(chip, ' ');
    }
    const actions = el('div', '');
    actions.append(button(tr('memory.export'), exportAll, 'settings-ghost'), ' ', button(tr('memory.forgetAll'), () => forgetAll(section), 'settings-ghost'));
    tabs.append(chips, actions);
    block.append(tabs);
    const head = el('div', 'cedar-set-block-head');
    head.append(el('span', 'settings-eyebrow', tr('memory.count', { count: data.items.length })));
    block.append(head);
    if (data.items.length) {
      const list = el('div', 'cedar-set-models');
      // Newest first here; the agent reads them oldest first.
      for (const item of [...data.items].reverse()) list.append(row(item, section));
      block.append(list);
    } else {
      block.append(el('p', 'cedar-set-note', tr(view === 'about' ? 'memory.empty' : `memory.empty.${view}`)));
    }
    parts.push(block, el('p', 'cedar-set-note', tr('memory.privacy')));
    section.replaceChildren(...parts);
  }

  async function reload(section) {
    const v = VIEWS[view];
    const params = new URLSearchParams();
    if (v.kinds.length) params.set('kind', v.kinds.join(','));
    if (v.archived) params.set('archived', 'true');
    try {
      const [memory, all, prefs] = await Promise.all([
        api('GET', `/memory?${params}`),
        api('GET', `/memory?${new URLSearchParams({ kind: ALL_KINDS.join(',') })}`).catch(() => null),
        api('GET', '/settings').catch(() => null),
      ]);
      data = memory;
      total = all && all.supported ? all.items.length : null;
      tools = prefs ? prefs.tools : undefined;
    } catch (err) {
      section.replaceChildren(el('p', 'cedar-set-error', tr('memory.loadFailed', { error: err.message })));
      return;
    }
    draw(section);
  }

  async function render(section) {
    shell()?.setStatus('memory', '');
    if (!data) section.replaceChildren(el('p', 'cedar-set-note', tr('memory.loading')));
    await reload(section);
  }

  function summary() {
    if (!data) return null;
    if (!data.supported) return tr('memory.summaryUnsupported');
    if (offInTools()) return { text: tr('memory.summaryOff'), kind: 'warn' };
    return data.profile ? tr('memory.summaryProfile') : tr('memory.summary', { count: total ?? data.items.length });
  }

  function bind() {
    if (!shell()) return;
    const body = document.getElementById('settings-body');
    if (!body) return;
    let section = body.querySelector('.settings-page[data-page="memory"]');
    if (!section) {
      section = el('section', 'settings-page');
      section.dataset.page = 'memory';
      section.id = 'memory-section';
      section.hidden = true;
      body.append(section);
    }
    shell().register({
      id: 'memory',
      label: tr('memory.title'),
      title: tr('memory.title'),
      description: tr('memory.description'),
      render,
      summary,
      footer: false,
    });
  }

  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', bind);
  else bind();
})();

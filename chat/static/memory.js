'use strict';

// Settings → Memory: what Cedar remembers about the user across conversations (cedar's
// `/v1/memory`, through `/api/memory`). Each fact is listed with when it was saved and the
// conversation it came from, with a Remove button; Forget all clears the list. What is listed is
// exactly what the agent is shown, and a removal applies from the next message on, so there is
// nothing to save. The page draws its own section, like the Language page, and needs only its
// script tag after settings-shell.js.

(() => {
  let data = null;      // the last GET /api/memory: { supported, items, limits }
  let tools = undefined; // the saved default tool selection (null: every group)

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
    if (!res.ok) throw new Error(body?.error?.message || tr('memory.requestFailed', { status: String(res.status) }));
    return body;
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
    // No run: added by hand, or its run was deleted with its conversation.
    return el('span', '', item.run_id ? tr('memory.fromElsewhere') : tr('memory.noSource'));
  }

  function row(item, section) {
    const r = el('div', 'cedar-set-integration');
    const main = el('div', 'cedar-set-integration-main');
    const sub = el('small', '');
    sub.append(`${when(item.created_at)} · `, source(item));
    main.append(el('span', '', item.text), sub);
    const side = el('div', 'cedar-set-integration-side');
    side.append(button(tr('memory.remove'), async (e) => {
      e.currentTarget.disabled = true;
      try {
        await api('DELETE', `/memory/${encodeURIComponent(item.id)}`);
        data.items = data.items.filter((i) => i.id !== item.id);
      } catch (err) {
        shell()?.setStatus('memory', err.message, 'error');
      }
      draw(section);
    }));
    r.append(main, side);
    return r;
  }

  async function forgetAll(section) {
    const count = data?.items?.length || 0;
    if (!count) return;
    if (!(await cedarConfirm(tr('memory.confirmForgetAll', { count }), { ok: tr('memory.forgetAll') }))) return;
    try {
      await api('DELETE', '/memory');
      data.items = [];
    } catch (err) {
      shell()?.setStatus('memory', err.message, 'error');
    }
    draw(section);
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
    const block = el('div', 'cedar-set-block');
    const head = el('div', 'cedar-set-block-head');
    const max = data.limits?.max_items;
    head.append(el('span', 'settings-eyebrow', max
      ? tr('memory.countOf', { count: data.items.length, max })
      : tr('memory.count', { count: data.items.length })));
    if (data.items.length) head.append(button(tr('memory.forgetAll'), () => forgetAll(section), 'settings-ghost'));
    block.append(head);
    if (data.items.length) {
      const list = el('div', 'cedar-set-models');
      // Newest first here; the agent reads them oldest first.
      for (const item of [...data.items].reverse()) list.append(row(item, section));
      block.append(list);
    } else {
      block.append(el('p', 'cedar-set-note', tr('memory.empty')));
    }
    parts.push(block, el('p', 'cedar-set-note', tr('memory.privacy')));
    section.replaceChildren(...parts);
  }

  async function render(section) {
    shell()?.setStatus('memory', '');
    if (!data) section.replaceChildren(el('p', 'cedar-set-note', tr('memory.loading')));
    try {
      const [memory, prefs] = await Promise.all([api('GET', '/memory'), api('GET', '/settings').catch(() => null)]);
      data = memory;
      tools = prefs ? prefs.tools : undefined;
    } catch (err) {
      section.replaceChildren(el('p', 'cedar-set-error', tr('memory.loadFailed', { error: err.message })));
      return;
    }
    draw(section);
  }

  function summary() {
    if (!data) return null;
    if (!data.supported) return tr('memory.summaryUnsupported');
    if (offInTools()) return { text: tr('memory.summaryOff'), kind: 'warn' };
    return tr('memory.summary', { count: data.items.length });
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

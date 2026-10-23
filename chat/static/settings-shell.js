'use strict';

// The Settings dialog's frame, shared by every UI: a left nav with one entry per page, each
// showing its current value, one page at a time on the right, and a footer with Save for the
// pages that have something to save. The pages themselves come from the other modules, which
// register here:
//
//   cedarSettingsShell.register({
//     id, label,            // 'interface' | 'tools' | 'model' | 'notify' | 'iphone' | 'server', and the nav name
//     title, description,   // the page head
//     render(sectionEl),    // draw the page; called every time the dialog opens
//     summary() -> string | {text, kind} | null   the nav's second line (null: none yet, shows "…")
//     tag() -> string | null                      a small mono tag instead (the Server item)
//     actions() -> Node[] | null                  buttons for the page head (the Server page's Lock)
//     footer: boolean | () => boolean             whether the page saves through the footer
//     save() -> Promise                           what the footer's Save does
//   })
//
// Pages report their state with setDirty(id, count) and setStatus(id, text, kind), and call
// refresh(id) when their summary changed. A click on #settings-btn opens the dialog on the page
// last used in this session, or on the one named in the button's data-page (the inbox's "Change
// in Settings" asks for 'notify', the Google redirect for 'server').

const cedarSettingsShell = (() => {
  const ORDER = ['interface', 'tools', 'model', 'notify', 'iphone', 'server'];
  const PAGE_KEY = 'cedar-chat-settings-page';

  const pages = new Map();
  const dirty = new Map();    // id -> 0 | n | true
  const notes = new Map();    // id -> { text, kind }
  let dialog = null;
  let navList = null;
  let navItems = new Map();   // id -> button
  let body = null;
  let titleEl = null;
  let descEl = null;
  let actsEl = null;
  let foot = null;
  let statusEl = null;
  let saveBtn = null;
  let current = null;
  let saving = false;
  let bound = false;

  function el(tag, className, text) {
    const e = document.createElement(tag);
    if (className) e.className = className;
    if (text !== undefined) e.textContent = text;
    return e;
  }

  function sectionOf(id) {
    return dialog?.querySelector(`.settings-page[data-page="${id}"]`) || null;
  }

  function lastPage() {
    try { return sessionStorage.getItem(PAGE_KEY) || ''; } catch { return ''; }
  }

  function rememberPage(id) {
    try { sessionStorage.setItem(PAGE_KEY, id); } catch { /* then the next open starts at Interface */ }
  }

  // -- the nav -----------------------------------------------------------------------------------

  function navItem(page) {
    const b = el('button', 'settings-nav-item');
    b.type = 'button';
    b.dataset.page = page.id;
    b.append(el('span', 'settings-nav-name', page.label), el('span', 'settings-nav-value', '…'));
    b.addEventListener('click', () => go(page.id));
    return b;
  }

  function buildNav() {
    if (!navList) return;
    navList.textContent = '';
    navItems = new Map();
    for (const id of ORDER) {
      const page = pages.get(id);
      if (!page) continue;
      const item = id === 'server' ? dialog.querySelector('#settings-nav-server') : navItem(page);
      if (!item) continue;
      if (id === 'server' && !item.dataset.bound) {
        item.dataset.bound = '1';
        item.addEventListener('click', () => go('server'));
      }
      navItems.set(id, item);
      if (id !== 'server') navList.append(item);
    }
  }

  /** Recompute one page's (or every page's) nav line, tag and head actions. */
  function refresh(id) {
    const ids = id ? [id] : [...pages.keys()];
    for (const pid of ids) {
      const page = pages.get(pid);
      const item = navItems.get(pid);
      if (!page || !item) continue;
      const value = item.querySelector('.settings-nav-value');
      if (value) {
        let s = null;
        try { s = page.summary ? page.summary() : null; } catch { s = null; }
        const text = s && typeof s === 'object' ? s.text : s;
        const kind = s && typeof s === 'object' ? s.kind || '' : '';
        value.textContent = text ?? '…';
        value.className = `settings-nav-value${kind ? ` ${kind}` : ''}`;
      }
      const tagEl = item.querySelector('.settings-nav-tag');
      if (tagEl && page.tag) {
        const t = page.tag();
        tagEl.textContent = t || '';
        tagEl.classList.toggle('on', t === 'UNLOCKED');
      }
      if (pid === current) {
        renderActions(page);
        updateFoot();
      }
    }
  }

  function renderActions(page) {
    if (!actsEl) return;
    actsEl.textContent = '';
    const nodes = page.actions ? page.actions() : null;
    if (nodes?.length) actsEl.append(...nodes);
  }

  // -- pages -------------------------------------------------------------------------------------

  function go(id) {
    const page = pages.get(id) || pages.get('interface');
    if (!page || !dialog) return;
    current = page.id;
    rememberPage(page.id);
    for (const section of dialog.querySelectorAll('.settings-page')) section.hidden = section.dataset.page !== page.id;
    for (const [pid, item] of navItems) {
      if (pid === page.id) item.setAttribute('aria-current', 'page');
      else item.removeAttribute('aria-current');
    }
    if (titleEl) titleEl.textContent = page.title || page.label;
    if (descEl) descEl.textContent = page.description || '';
    renderActions(page);
    if (body) body.scrollTop = 0;
    updateFoot();
  }

  function hasFooter(page) {
    if (!page) return false;
    return typeof page.footer === 'function' ? Boolean(page.footer()) : page.footer !== false;
  }

  function updateFoot() {
    if (!foot) return;
    const page = pages.get(current);
    foot.hidden = !hasFooter(page);
    if (foot.hidden) return;
    const d = dirty.get(current) || 0;
    const note = notes.get(current);
    let text = '';
    if (saving) text = 'Saving…';
    else if (d === true) text = 'Unsaved changes';
    else if (typeof d === 'number' && d > 0) text = `${d} change${d === 1 ? '' : 's'}`;
    else if (statusEl?.dataset.saved === current) text = 'Saved';
    const parts = [text, note?.text].filter(Boolean);
    statusEl.textContent = parts.join(' · ');
    statusEl.className = `cedar-set-status${note?.kind ? ` ${note.kind}` : ''}`;
    saveBtn.disabled = saving || !d;
  }

  function setDirty(id, count) {
    dirty.set(id, count === true ? true : Number(count) || 0);
    if (statusEl && count) delete statusEl.dataset.saved;
    if (id === current) updateFoot();
  }

  function setStatus(id, text, kind = '') {
    if (text) notes.set(id, { text, kind });
    else notes.delete(id);
    if (id === current) updateFoot();
  }

  async function save() {
    const page = pages.get(current);
    if (!page?.save || saving) return;
    saving = true;
    updateFoot();
    try {
      await page.save();
      dirty.set(page.id, 0);
      if (statusEl) statusEl.dataset.saved = page.id;
      refresh(page.id);
    } catch { /* the page reports it */ } finally {
      saving = false;
      updateFoot();
    }
  }

  // -- opening -----------------------------------------------------------------------------------

  function open(pageId) {
    if (!dialog) return;
    if (statusEl) delete statusEl.dataset.saved;
    for (const id of pages.keys()) { dirty.set(id, 0); notes.delete(id); }
    if (!dialog.open) dialog.showModal();
    buildNav();
    const start = pages.has(pageId) ? pageId : pages.has(lastPage()) ? lastPage() : 'interface';
    go(start);
    for (const page of pages.values()) {
      const section = sectionOf(page.id);
      if (section && page.render) {
        try { Promise.resolve(page.render(section)).then(() => refresh(page.id)).catch(() => {}); } catch { /* the page reports it */ }
      }
    }
    refresh();
  }

  function onNavKey(e) {
    if (e.key !== 'ArrowDown' && e.key !== 'ArrowUp') return;
    const items = [...navItems.values()];
    const i = items.indexOf(document.activeElement);
    if (i < 0) return;
    e.preventDefault();
    const next = items[(i + (e.key === 'ArrowDown' ? 1 : items.length - 1)) % items.length];
    next.focus();
  }

  function bind() {
    dialog = document.getElementById('settings-dialog');
    const openBtn = document.getElementById('settings-btn');
    if (!dialog || !openBtn) return;
    navList = dialog.querySelector('#settings-nav');
    body = dialog.querySelector('#settings-body');
    titleEl = dialog.querySelector('#settings-page-title');
    descEl = dialog.querySelector('#settings-page-desc');
    actsEl = dialog.querySelector('#settings-page-acts');
    foot = dialog.querySelector('#settings-foot');
    statusEl = dialog.querySelector('#settings-status');
    saveBtn = dialog.querySelector('#settings-save');
    openBtn.addEventListener('click', () => {
      const wanted = openBtn.dataset.page || '';
      delete openBtn.dataset.page;
      open(wanted);
    });
    dialog.querySelector('#settings-close')?.addEventListener('click', () => dialog.close());
    saveBtn?.addEventListener('click', save);
    dialog.querySelector('.settings-nav')?.addEventListener('keydown', onNavKey);
    document.addEventListener('cedar-models-changed', () => { if (dialog.open) refresh(); });
    bound = true;
    buildNav();
  }

  function register(page) {
    if (!page?.id) return;
    pages.set(page.id, page);
    if (bound) buildNav();
  }

  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', bind);
  else bind();

  return { register, go, open, refresh, setDirty, setStatus, isOpen: () => Boolean(dialog?.open), current: () => current };
})();

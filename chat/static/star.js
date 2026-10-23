'use strict';

// Starred conversations, shared by every UI. Two pieces:
//
//   - A star button (every `[data-star-toggle]` the page has) that stars or unstars the open
//     conversation, a chat or a scheduled run alike (PUT / DELETE /api/conversations/{id}/star).
//   - The "Starred" list in the conversations sidebar, drawn into each `[data-starred]` mount
//     (GET /api/starred). Stars live on the server, so the list is the same in every browser, the
//     desktop app and the iOS app; it is reloaded when the page comes back into view, and every
//     minute while it is in view, to pick up stars set elsewhere.
//
// Each app hands over its own helpers through cedarStar.init(), calls cedarStar.start() once the
// access token is known, and cedarStar.setCurrent(conversation) whenever the open conversation
// changes or is renamed:
//
//   api(method, path, body) -> Promise<json>   the app's /api client (sends the access token)
//   toast(message)                             a short notice
//   onOpen()                                   optional: a link in the list was clicked (close a drawer)

const cedarStar = (() => {
  const REFRESH_MS = 60_000;
  const STAR = '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M12 3.5l2.6 5.3 5.9.9-4.2 4.1 1 5.8L12 16.9l-5.3 2.7 1-5.8-4.2-4.1 5.9-.9z"/></svg>';
  const CLOCK = '<svg viewBox="0 0 24 24" aria-hidden="true"><circle cx="12" cy="12" r="9"/><path d="M12 7v5l3 2"/></svg>';

  let deps = null;
  let starred = [];      // GET /starred, as last loaded
  let current = null;    // the open conversation, as the app last handed it over
  let started = false;
  let request = 0;       // bumped on every load so a slow fetch can't overwrite a newer list
  let timer = null;

  function el(tag, className, text) {
    const e = document.createElement(tag);
    if (className) e.className = className;
    if (text !== undefined) e.textContent = text;
    return e;
  }

  function isStarred(id) {
    return starred.some((c) => c.id === id);
  }

  function title(c) {
    return c.title || 'New chat';
  }

  // -- the sidebar list --------------------------------------------------------------------------

  /** Draw the starred conversations into every sidebar mount (`[data-starred]`); hidden when there are none. */
  function renderSide() {
    for (const mount of document.querySelectorAll('[data-starred]')) {
      mount.hidden = !starred.length;
      const count = mount.querySelector('.cedar-star-count');
      if (count) count.textContent = String(starred.length);
      const list = mount.querySelector('.cedar-star-links');
      if (!list) continue;
      list.textContent = '';
      for (const c of starred) {
        const link = el('a', 'cedar-star-link');
        link.href = `#/c/${c.id}`;
        link.classList.toggle('active', current?.id === c.id);
        link.classList.toggle('is-unread', Boolean(c.unread));
        link.title = c.schedule_title ? `${title(c)} · scheduled by ${c.schedule_title}` : title(c);
        const name = el('span', 'cedar-star-link-name');
        // A scheduled run gets the clock the schedule list uses, so the two kinds are told apart.
        name.insertAdjacentHTML('beforeend', c.schedule_id ? CLOCK : STAR);
        name.append(el('span', 'cedar-star-link-title', title(c)));
        link.append(name);
        if (c.needs_answer) link.append(el('span', 'cedar-star-link-note', 'Needs answer'));
        else if (c.unread) link.append(el('span', 'cedar-star-link-dot'));
        link.addEventListener('click', () => deps.onOpen?.());
        list.append(link);
      }
    }
  }

  async function load() {
    const mine = ++request;
    try {
      const res = await deps.api('GET', '/starred');
      if (mine !== request) return;
      starred = res.conversations || [];
    } catch {
      return; // keep the last list; the next refresh tries again
    }
    render();
  }

  // -- the star button ---------------------------------------------------------------------------

  function renderButtons() {
    const on = Boolean(current && isStarred(current.id));
    for (const b of document.querySelectorAll('[data-star-toggle]')) {
      b.hidden = !current;
      b.classList.add('cedar-star-btn');
      b.setAttribute('aria-pressed', String(on));
      const label = on ? 'Unstar this conversation' : 'Star this conversation';
      b.setAttribute('aria-label', label);
      b.title = on ? 'Starred: listed under Starred on every device. Click to unstar' : 'Star: list this conversation under Starred on every device';
      const lbl = b.querySelector('.lbl');
      if (lbl) lbl.textContent = on ? 'Starred' : 'Star';
    }
  }

  function render() {
    renderSide();
    renderButtons();
  }

  async function toggle(button) {
    if (!current) return;
    const c = current;
    const on = !isStarred(c.id);
    // Shown at once; put back if the server refuses.
    const before = starred;
    starred = on ? [{ ...c, starred_at: new Date().toISOString(), schedule_title: null }, ...starred] : starred.filter((x) => x.id !== c.id);
    render();
    button.disabled = true;
    try {
      await deps.api(on ? 'PUT' : 'DELETE', `/conversations/${c.id}/star`);
      deps.toast(on ? 'Starred' : 'Removed from Starred');
      load(); // for the schedule's title, and anything starred elsewhere in the meantime
    } catch (e) {
      starred = before;
      render();
      deps.toast(e.message || 'Could not change the star');
    } finally {
      button.disabled = false;
    }
  }

  // -- wiring ------------------------------------------------------------------------------------

  function bind() {
    for (const b of document.querySelectorAll('[data-star-toggle]')) {
      if (!b.querySelector('svg')) b.insertAdjacentHTML('afterbegin', STAR);
      b.addEventListener('click', () => toggle(b));
    }
    renderButtons();
  }

  function schedule() {
    clearInterval(timer);
    timer = document.visibilityState === 'visible' ? setInterval(load, REFRESH_MS) : null;
  }

  function init(helpers) {
    deps = helpers;
    if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', bind);
    else bind();
  }

  /** Load the list; once, after the access token is known. */
  function start() {
    if (started || !deps) return;
    started = true;
    load();
    schedule();
    document.addEventListener('visibilitychange', () => {
      if (document.visibilityState === 'visible') load();
      schedule();
    });
    // A deleted conversation leaves through a navigation; reload so it leaves the list too.
    window.addEventListener('hashchange', () => { if (!location.hash.startsWith('#/c/')) load(); });
  }

  /** The open conversation changed, or was renamed (`null` when none is open). */
  function setCurrent(conversation) {
    const before = current?.id;
    current = conversation || null;
    // Apps call this on every repaint of their header; the list is redrawn only when it changes.
    const listed = current && starred.find((c) => c.id === current.id);
    const renamed = listed && current.title && listed.title !== current.title;
    if (renamed) listed.title = current.title;
    if (renamed || before !== current?.id) renderSide();
    renderButtons();
  }

  return { init, start, setCurrent, refresh: load, isStarred };
})();

'use strict';

// Being told when a response finishes, shared by both UIs (/v1 and /v2). Three pieces:
//
//   - An inbox: the bell button in the header counts the responses that finished (or failed)
//     while nobody was looking, scheduled runs included, and opens a list of them; the count is
//     also put in the page title. A row opens its conversation, which marks it seen.
//   - Live notices from GET /api/events, a `turn.finished` event for every response that ends as
//     cedar tells the server: the inbox and the app's conversation list follow at once, a toast
//     says what finished in another conversation, and, if turned on in the inbox, a browser
//     notification is shown while the tab is hidden.
//   - "Seen": the app says when the open conversation has been looked at (it was opened, its
//     response finished while the tab was visible), this module adds the tab becoming visible
//     again, and the server clears the marks.
//
// The app hands over its helpers through cedarNotify.init():
//
//   api(method, path, body) -> Promise<json>   the app's /api client (sends the access token)
//   authHeaders() -> object                    the access token header, for the event stream
//   toast(message)                             a short notice
//   button                                     the bell button in the header
//   current() -> conversation | null           the open conversation
//   onChange()                                 the marks changed: reload the conversation list
//
// and calls cedarNotify.start() once the access token is known, cedarNotify.setTitle(text)
// instead of setting document.title, and cedarNotify.seen(conversationId) when a conversation
// has been looked at.

const cedarNotify = (() => {
  const PREF_KEY = 'cedar-chat-notify'; // '1' when browser notifications are wanted

  let deps = null;
  let inbox = { items: [], unread: 0 };
  let baseTitle = document.title;
  let countEl = null;
  let dialog = null;
  let listEl = null;
  let allRead = null;
  let toggle = null;
  let started = false;
  const announced = new Set(); // turn ids already told about, so a reconnect doesn't repeat them

  // -- helpers -----------------------------------------------------------------------------------

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

  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

  function wantsBrowser() {
    try { return localStorage.getItem(PREF_KEY) === '1'; } catch { return false; }
  }

  function setWantsBrowser(on) {
    try { if (on) localStorage.setItem(PREF_KEY, '1'); else localStorage.removeItem(PREF_KEY); } catch { /* this visit only */ }
  }

  /** "Finished", "Failed" or "Needs your answer", as the inbox and the notices say it. */
  function kindOf(item) {
    if (item.needs_answer) return 'Needs your answer';
    return item.status === 'failed' ? 'Failed' : 'Finished';
  }

  function when(iso) {
    if (!iso) return '';
    const d = new Date(iso);
    const today = new Date().toDateString() === d.toDateString();
    return today ? d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })
      : d.toLocaleString([], { weekday: 'short', day: 'numeric', month: 'short', hour: '2-digit', minute: '2-digit' });
  }

  const nameOf = (item) => item.title || 'New chat';

  // -- the count and the title -------------------------------------------------------------------

  function applyTitle() {
    document.title = inbox.unread ? `(${inbox.unread}) ${baseTitle}` : baseTitle;
  }

  function setTitle(text) {
    baseTitle = text;
    applyTitle();
  }

  function updateBadge() {
    applyTitle();
    if (!countEl) return;
    countEl.hidden = !inbox.unread;
    countEl.textContent = inbox.unread > 99 ? '99+' : String(inbox.unread);
    const label = inbox.unread ? `Finished responses, ${inbox.unread} unread` : 'Finished responses';
    deps.button.setAttribute('aria-label', label);
    deps.button.title = label;
  }

  async function refresh() {
    try { inbox = await deps.api('GET', '/inbox'); } catch { return; }
    updateBadge();
    if (dialog?.open) renderList();
  }

  const unreadIn = (conversationId) => inbox.items.some((i) => i.conversation_id === conversationId);

  /** The conversation has been looked at: its finished responses leave the inbox. */
  async function seen(conversationId) {
    if (!conversationId || !deps) return;
    const before = inbox.items.length;
    inbox.items = inbox.items.filter((i) => i.conversation_id !== conversationId);
    inbox.unread = Math.max(0, inbox.unread - (before - inbox.items.length));
    updateBadge();
    try { await deps.api('POST', `/conversations/${conversationId}/seen`); } catch { return; }
    deps.onChange?.();
    refresh();
  }

  // -- the inbox dialog --------------------------------------------------------------------------

  function ensureDialog() {
    if (dialog) return dialog;
    dialog = el('dialog', 'cedar-inbox-dialog');
    dialog.setAttribute('aria-labelledby', 'cedar-inbox-title');
    const head = el('div', 'cedar-inbox-head');
    const title = el('h3', '', 'Finished responses');
    title.id = 'cedar-inbox-title';
    allRead = button('Mark all read', async () => {
      const ids = [...new Set(inbox.items.map((i) => i.conversation_id))];
      for (const id of ids) await seen(id);
    });
    head.append(title, allRead, button('Close', () => dialog.close()));
    listEl = el('div', 'cedar-inbox-list');
    const foot = el('div', 'cedar-inbox-foot');
    const label = el('label');
    toggle = el('input');
    toggle.type = 'checkbox';
    toggle.checked = wantsBrowser() && typeof Notification !== 'undefined' && Notification.permission === 'granted';
    toggle.disabled = typeof Notification === 'undefined';
    toggle.addEventListener('change', async () => {
      if (!toggle.checked) { setWantsBrowser(false); return; }
      let permission = Notification.permission;
      if (permission === 'default') {
        try { permission = await Notification.requestPermission(); } catch { permission = 'denied'; }
      }
      if (permission !== 'granted') {
        toggle.checked = false;
        deps.toast('The browser does not allow notifications for this site');
        return;
      }
      setWantsBrowser(true);
    });
    label.append(toggle, el('span', '', typeof Notification === 'undefined'
      ? 'This browser cannot show notifications.'
      : 'Also show a browser notification when a response finishes while this tab is hidden.'));
    foot.append(label);
    dialog.append(head, listEl, foot);
    document.body.append(dialog);
    return dialog;
  }

  function renderList() {
    listEl.textContent = '';
    allRead.hidden = !inbox.items.length;
    if (!inbox.items.length) {
      listEl.append(el('div', 'cedar-inbox-empty', 'Nothing new. Responses that finish while you are elsewhere show up here.'));
      return;
    }
    for (const item of inbox.items) {
      const row = el('button', 'cedar-inbox-item');
      row.type = 'button';
      const dot = el('span', `cedar-inbox-dot${item.needs_answer ? ' ask' : item.status === 'failed' ? ' err' : ''}`);
      const text = el('span', 'cedar-inbox-text');
      text.append(el('span', 'cedar-inbox-title', nameOf(item)));
      const parts = [kindOf(item)];
      if (item.schedule_id) parts.push('scheduled run');
      parts.push(when(item.finished_at));
      text.append(el('span', 'cedar-inbox-sub', parts.join(' · ')));
      row.append(dot, text);
      row.addEventListener('click', () => {
        dialog.close();
        if (deps.current()?.id === item.conversation_id) seen(item.conversation_id);
        else location.hash = `#/c/${item.conversation_id}`;
      });
      listEl.append(row);
    }
  }

  function open() {
    ensureDialog();
    renderList();
    dialog.showModal();
    refresh();
  }

  // -- live notices ------------------------------------------------------------------------------

  function parseSse(raw) {
    let data = '';
    for (const line of raw.split('\n')) if (line.startsWith('data:')) data += line.slice(line[5] === ' ' ? 6 : 5);
    if (!data) return null;
    try { return JSON.parse(data); } catch { return null; }
  }

  function describe(notice) {
    return `${kindOf(notice)}: ${nameOf(notice)}`;
  }

  function showBrowserNotification(notice) {
    if (!wantsBrowser() || typeof Notification === 'undefined' || Notification.permission !== 'granted') return;
    try {
      const n = new Notification(describe(notice), {
        body: notice.schedule_id ? 'A scheduled run of Cedar' : 'Cedar',
        tag: `cedar-turn-${notice.turn_id}`,
      });
      n.addEventListener('click', () => {
        window.focus();
        location.hash = `#/c/${notice.conversation_id}`;
        n.close();
      });
    } catch { /* some browsers only allow it from a service worker */ }
  }

  function onNotice(notice) {
    if (!notice.turn_id || announced.has(notice.turn_id)) return;
    announced.add(notice.turn_id);
    const visible = document.visibilityState === 'visible';
    const mine = deps.current()?.id === notice.conversation_id;
    // The open conversation's own stream showed the response end; anything else is news.
    if (!(mine && visible)) {
      if (visible) deps.toast(describe(notice));
      else showBrowserNotification(notice);
    }
    refresh();
    deps.onChange?.();
  }

  async function follow() {
    let failures = 0;
    for (;;) {
      try {
        const res = await fetch('/api/events', { headers: deps.authHeaders() });
        if (res.status === 401) { await sleep(5000); continue; } // the app asks for the token
        if (!res.ok) throw new Error(`events ${res.status}`);
        failures = 0;
        refresh(); // what finished while the stream was down
        const reader = res.body.getReader();
        const decoder = new TextDecoder();
        let buf = '';
        for (;;) {
          const { value, done } = await reader.read();
          if (done) break;
          buf += decoder.decode(value, { stream: true });
          let cut;
          while ((cut = buf.indexOf('\n\n')) >= 0) {
            const notice = parseSse(buf.slice(0, cut));
            buf = buf.slice(cut + 2);
            if (notice?.type === 'turn.finished') onNotice(notice);
          }
        }
      } catch { failures++; }
      await sleep(Math.min(1000 * 2 ** failures, 30_000));
    }
  }

  // -- wiring ------------------------------------------------------------------------------------

  function init(d) {
    deps = d;
    deps.button.classList.add('cedar-inbox-btn');
    countEl = el('span', 'cedar-inbox-count');
    countEl.hidden = true;
    countEl.setAttribute('aria-hidden', 'true');
    deps.button.append(countEl);
    deps.button.addEventListener('click', open);
    document.addEventListener('visibilitychange', async () => {
      if (document.visibilityState !== 'visible') return;
      await refresh();
      const current = deps.current();
      if (current && unreadIn(current.id)) seen(current.id);
    });
  }

  /** Read the inbox and follow the notices; once, after the access token is known. */
  function start() {
    if (started || !deps) return;
    started = true;
    refresh();
    follow();
  }

  return { init, start, setTitle, seen, refresh, unread: () => inbox.unread };
})();

'use strict';

// Being told when a response finishes, shared by every UI. Four pieces:
//
//   - An inbox: the bell button in the header counts the responses that finished (or failed)
//     while nobody was looking, scheduled runs included, and opens a list of them; the count is
//     also put in the page title. A row opens its conversation, which marks it seen.
//   - Live notices from GET /api/events, a `turn.finished` event for every response that ends as
//     cedar tells the server: the inbox and the app's conversation list follow at once, and a
//     toast says what finished in another conversation.
//   - "Seen": the app says when the open conversation has been looked at (it was opened, its
//     response finished while the tab was visible), this module adds the tab becoming visible
//     again, and the server clears the marks.
//   - The Notifications page of Settings (framed by settings-shell.js): the ways to be told out
//     of the app (a notification from the Cedar app for macOS, browser push through the service
//     worker at /sw.js, a phone push through Pushover, an email) as one list of methods, the
//     chosen one unfolding its credentials and a test send, and which responses are worth it
//     (GET/PUT /api/notifications). schedule.js asks cedarNotify for the set-up methods to offer
//     in a scheduled agent's own picker.
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
  const METHODS = [
    ['none', 'Only in the app', 'The inbox, the marks in the conversation list and the count in the title.'],
    ['desktop', 'Desktop notification', 'A notification from the Cedar app on this computer (Notification Center on a Mac, a toast on Windows), its window open or not.'],
    ['browser', 'Browser push', 'A notification from this browser even when the tab is closed. Needs https, or localhost.'],
    ['ios', 'iPhone push', 'A notification from the Cedar iOS app, through Apple\'s push service.'],
    ['pushover', 'Phone push via Pushover', 'A push to your phone through pushover.net and its app.'],
    ['email', 'Email', 'A mail through an SMTP server of yours.'],
  ];
  const METHOD_LABELS = Object.fromEntries(METHODS.map(([id, label]) => [id, label]));

  let deps = null;
  let inbox = { items: [], unread: 0 };
  let baseTitle = document.title;
  let countEl = null;
  let dialog = null;
  let listEl = null;
  let allRead = null;
  let awayLine = null;
  let section = null;   // the Settings section
  let status = null;    // GET /api/notifications, as last loaded
  let started = false;
  const announced = new Set(); // turn ids already told about, so a reconnect doesn't repeat them
  // Inside the Cedar desktop app (its window sets this): its web view can't take browser push.
  const inDesktopApp = window.__CEDAR_DESKTOP_APP__ === true;

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
    awayLine = el('span', '', 'While you are away: only in the app.');
    const change = button('Change in Settings', () => {
      dialog.close();
      const btn = document.getElementById('settings-btn');
      if (btn) { btn.dataset.page = 'notify'; btn.click(); }
    });
    foot.append(awayLine, change);
    dialog.append(head, listEl, foot);
    document.body.append(dialog);
    return dialog;
  }

  function updateAwayLine() {
    if (!awayLine) return;
    const method = status?.default_method || 'none';
    awayLine.textContent = method === 'none' ? 'While you are away: only in the app.' : `While you are away: ${METHOD_LABELS[method] || method}.`;
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
    updateAwayLine();
    dialog.showModal();
    refresh();
    if (!status) deps.api('GET', '/notifications').then((s) => { status = s; updateAwayLine(); }).catch(() => {});
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

  function onNotice(notice) {
    if (!notice.turn_id || announced.has(notice.turn_id)) return;
    announced.add(notice.turn_id);
    const visible = document.visibilityState === 'visible';
    const mine = deps.current()?.id === notice.conversation_id;
    // The open conversation's own stream showed the response end; anything else is news here.
    // A hidden tab leaves it to the chosen out-of-app method (the server sends that).
    if (!(mine && visible) && visible) deps.toast(describe(notice));
    refresh();
    deps.onChange?.();
  }

  // -- Settings: how to be told out of the app ---------------------------------------------------

  const note = (text, className = 'cedar-set-note') => el('p', className, text);

  function field(label, input, extra) {
    const f = el('label', 'cedar-set-field');
    const head = el('span', 'cedar-set-field-head');
    head.append(el('span', '', label));
    if (extra) head.append(extra);
    f.append(head, input);
    return f;
  }

  function input(type, value, placeholder, label) {
    const i = el('input');
    i.type = type;
    i.value = value || '';
    i.placeholder = placeholder || '';
    i.setAttribute('aria-label', label);
    i.autocomplete = 'off';
    i.spellcheck = false;
    return i;
  }

  const badge = (text, kind = '') => el('span', `cedar-set-badge ${kind}`, text);

  const pushSupported = () => 'serviceWorker' in navigator && 'PushManager' in window && typeof Notification !== 'undefined';

  /** The browser's own subscription, if it has one. */
  async function localSubscription() {
    if (!pushSupported()) return null;
    try {
      const reg = await navigator.serviceWorker.getRegistration('/');
      return reg ? await reg.pushManager.getSubscription() : null;
    } catch { return null; }
  }

  function toKey(base64url) {
    const padded = base64url.replace(/-/g, '+').replace(/_/g, '/') + '='.repeat((4 - (base64url.length % 4)) % 4);
    const raw = atob(padded);
    return Uint8Array.from(raw, (c) => c.charCodeAt(0));
  }

  /** Register the service worker, subscribe with the server's key, and tell the server. */
  async function turnOnPush() {
    if (!pushSupported()) throw new Error('This browser cannot take push notifications here (it needs https, or localhost).');
    let permission = Notification.permission;
    if (permission === 'default') permission = await Notification.requestPermission();
    if (permission !== 'granted') throw new Error('The browser does not allow notifications for this site.');
    if (!status?.browser?.public_key) status = await deps.api('GET', '/notifications');
    const reg = await navigator.serviceWorker.register('/sw.js', { scope: '/' });
    await navigator.serviceWorker.ready;
    const options = { userVisibleOnly: true, applicationServerKey: toKey(status.browser.public_key) };
    let sub;
    try {
      sub = await reg.pushManager.subscribe(options);
    } catch (e) {
      // A subscription made with an older key of this server: replace it.
      const old = await reg.pushManager.getSubscription();
      if (!old) throw e;
      await old.unsubscribe();
      sub = await reg.pushManager.subscribe(options);
    }
    status = await deps.api('POST', '/notifications/push/subscriptions', sub.toJSON());
  }

  async function turnOffPush() {
    const sub = await localSubscription();
    if (!sub) return;
    const endpoint = sub.endpoint;
    await sub.unsubscribe();
    status = await deps.api('DELETE', '/notifications/push/subscriptions', { endpoint });
  }

  function agentName(ua) {
    if (!ua) return 'A browser';
    const m = ua.match(/(Firefox|Edg|OPR|Chrome|Safari)\/[\d.]+/);
    const name = { Edg: 'Edge', OPR: 'Opera' }[m?.[1]] || m?.[1] || 'A browser';
    const os = /Android/.test(ua) ? 'Android' : /iPhone|iPad/.test(ua) ? 'iOS' : /Mac OS/.test(ua) ? 'macOS' : /Windows/.test(ua) ? 'Windows' : /Linux/.test(ua) ? 'Linux' : '';
    return os ? `${name} on ${os}` : name;
  }

  /** The settings as last loaded (or loaded now), for other modules' pickers. */
  async function loadStatus(force) {
    if (!status || force) status = await deps.api('GET', '/notifications');
    return status;
  }

  /**
   * Options for a "notify by" picker: the methods that are set up, each `{value, label}`, with
   * `none` first. `withDefault` adds a first entry that follows the default method in Settings.
   */
  function methodOptions(withDefault) {
    const configured = status?.configured || [];
    const options = [];
    if (withDefault) {
      const d = status?.default_method || 'none';
      options.push({ value: '', label: d === 'none' ? 'Default (only in the app)' : `Default (${METHOD_LABELS[d] || d})` });
    }
    options.push({ value: 'none', label: 'Only in the app' });
    for (const m of configured) options.push({ value: m, label: METHOD_LABELS[m] || m });
    return options;
  }

  let draft = null;        // { default_method, scope, pushover: {}, email: {}, ios: {}, contact? }
  let openMethod = null;   // which method's setup panel is unfolded
  let local = null;        // this browser's push subscription, if any

  const shell = () => (typeof cedarSettingsShell === 'undefined' ? null : cedarSettingsShell);

  /** The nav's second line: "Phone push via Pushover · what needs me". */
  function summary() {
    if (!status) return null;
    const m = status.default_method || 'none';
    return `${m === 'none' ? 'Only in the app' : METHOD_LABELS[m] || m} · ${status.scope === 'all' ? 'every response' : 'what needs me'}`;
  }

  /** The footer's warning while the chosen default isn't set up. */
  function guard() {
    const m = draft?.default_method || 'none';
    const ok = m === 'none' || (status?.configured || []).includes(m);
    shell()?.setStatus('notify', ok ? '' : `${METHOD_LABELS[m] || m} isn't set up yet`, 'warn');
  }

  async function renderSettings(sectionEl = section) {
    if (sectionEl) section = sectionEl;
    if (!section) return;
    section.replaceChildren(note('Loading…'));
    try {
      [status, local] = await Promise.all([deps.api('GET', '/notifications'), localSubscription()]);
    } catch (e) {
      section.replaceChildren(note(`Could not load the settings: ${e.message}`, 'cedar-set-error'));
      return;
    }
    updateAwayLine();
    // A re-render after a test or a Forget keeps what was picked but not yet saved.
    const kept = draft && shell()?.current() === 'notify' ? draft : null;
    draft = { default_method: kept?.default_method ?? status.default_method, scope: kept?.scope ?? status.scope, pushover: {}, email: {}, ios: {} };
    const configured = new Set(status.configured || []);
    const dirty = () => shell()?.setDirty('notify', true);
    const row = (label) => {
      const r = el('div', 'cedar-notify-row');
      r.append(el('div', 'settings-eyebrow', label));
      return r;
    };

    // Tell me by: one radio per method; the chosen one (or one being set up) unfolds its setup.
    const tell = row('Tell me by');
    const group = el('div', 'cedar-notify-methods');
    group.setAttribute('role', 'radiogroup');
    group.setAttribute('aria-label', 'Tell me by');
    const panels = new Map();
    const radios = new Map();
    const syncOpen = () => {
      for (const [id, panel] of panels) panel.hidden = openMethod !== id;
      for (const [id, radio] of radios) radio.setAttribute('aria-checked', String(draft.default_method === id));
    };
    for (const [id, label, sub] of METHODS) {
      // The desktop app's method only where there is one; browser push not in the app's web view.
      if (id === 'desktop' && !inDesktopApp && !status.desktop?.configured) continue;
      if (id === 'browser' && inDesktopApp) continue;
      const item = el('div', 'cedar-notify-method');
      const head = el('div', 'cedar-notify-method-head');
      const radio = el('button', 'cedar-notify-radio');
      radio.type = 'button';
      radio.setAttribute('role', 'radio');
      radio.setAttribute('aria-label', label);
      radios.set(id, radio);
      const text = el('span', 'cedar-notify-method-text');
      text.append(el('span', 'cedar-notify-method-label', label), el('span', 'cedar-notify-method-sub', sub));
      const side = el('span', 'cedar-notify-method-side');
      if (id === 'none') side.append(badge('Always on'));
      else if (configured.has(id)) side.append(badge('Set up', 'ok'));
      else {
        side.append(badge('Not set up'));
        const setUp = el('button', 'settings-link', 'Set up');
        setUp.type = 'button';
        setUp.addEventListener('click', () => { openMethod = openMethod === id ? null : id; syncOpen(); });
        side.append(setUp);
      }
      head.append(radio, text, side);
      item.append(head);
      const pick = () => {
        draft.default_method = id;
        openMethod = id === 'none' ? null : id;
        syncOpen();
        guard();
        dirty();
      };
      radio.addEventListener('click', pick);
      text.addEventListener('click', pick);
      if (id !== 'none') {
        const panel = el('div', 'cedar-notify-setup');
        if (id === 'desktop') panel.append(...desktopBlock());
        if (id === 'browser') panel.append(...browserBlock(local));
        if (id === 'ios') panel.append(...iosBlock());
        if (id === 'pushover') panel.append(...pushoverBlock());
        if (id === 'email') panel.append(...emailBlock());
        panels.set(id, panel);
        item.append(panel);
      }
      group.append(item);
    }
    if (!radios.has(draft.default_method)) {
      // Picked once, no longer offered (the desktop app went away): shown so it can be changed.
      const m = draft.default_method;
      const item = el('div', 'cedar-notify-method');
      const head = el('div', 'cedar-notify-method-head');
      const radio = el('button', 'cedar-notify-radio');
      radio.type = 'button';
      radio.setAttribute('role', 'radio');
      radios.set(m, radio);
      const text = el('span', 'cedar-notify-method-text');
      text.append(el('span', 'cedar-notify-method-label', METHOD_LABELS[m] || m), el('span', 'cedar-notify-method-sub', 'No longer set up.'));
      head.append(radio, text, badge('Not set up'));
      item.append(head);
      group.append(item);
    }
    syncOpen();
    tell.append(group);

    // For: which responses are worth it.
    const scopeRow = row('For');
    const scopeWrap = el('div', 'cedar-notify-scope');
    const seg = el('div', 'cedar-notify-seg');
    seg.setAttribute('role', 'radiogroup');
    seg.setAttribute('aria-label', 'Which responses');
    const scopeNote = el('div', 'cedar-set-note');
    const SCOPE_NOTES = {
      attention: 'Failures, questions and scheduled runs. Scheduled agents can pick their own method.',
      all: 'Every response that ends while you are not watching it.',
    };
    const segs = [];
    const syncScope = () => {
      for (const b of segs) b.setAttribute('aria-checked', String(b.dataset.scope === draft.scope));
      scopeNote.textContent = SCOPE_NOTES[draft.scope] || SCOPE_NOTES.attention;
    };
    for (const [id, label] of [['attention', 'What needs me'], ['all', 'Every response']]) {
      const b = el('button', 'cedar-notify-seg-item', label);
      b.type = 'button';
      b.setAttribute('role', 'radio');
      b.dataset.scope = id;
      b.addEventListener('click', () => { draft.scope = id; syncScope(); dirty(); });
      segs.push(b);
      seg.append(b);
    }
    syncScope();
    scopeWrap.append(seg, scopeNote);
    scopeRow.append(scopeWrap);

    // Last sent.
    const lastRow = row('Last sent');
    lastRow.classList.add('last');
    const last = el('div', 'cedar-notify-last');
    if (status.last_delivery) {
      const d = status.last_delivery;
      last.classList.toggle('err', !d.ok);
      last.append(el('span', 'cedar-notify-when', when(d.at)));
      last.append(` · ${d.test ? 'test · ' : ''}${METHOD_LABELS[d.method] || d.method} · ${d.ok ? 'sent' : 'failed'}${d.detail ? ` · ${d.detail}` : ''}`);
    } else {
      last.textContent = 'Nothing has been sent yet.';
    }
    lastRow.append(last);

    section.replaceChildren(tell, scopeRow, lastRow);
    guard();

    /** Save, then send a test by `method`; the outcome lands in the last-sent line. */
    function testButton(method) {
      const b = button('Send a test', async () => {
        b.disabled = true;
        try {
          await submit();
          const result = await deps.api('POST', '/notifications/test', { method });
          deps.toast(`Sent: ${result.detail}`);
        } catch (e) {
          if (e.message) deps.toast(e.message);
        } finally {
          b.disabled = false;
          renderSettings();
        }
      }, 'settings-ghost');
      return b;
    }

    /** The help sentence left, the buttons right. */
    function helpRow(text, ...buttons) {
      const r = el('div', 'cedar-notify-help');
      r.append(el('span', '', text), ...buttons);
      return r;
    }

    function desktopBlock() {
      const d = status.desktop || {};
      if (d.listening) return [helpRow('The Cedar app is running and will show them. If nothing appears, allow notifications for Cedar in System Settings → Notifications on a Mac, or Settings → System → Notifications on Windows.', testButton('desktop'))];
      if (d.configured) return [helpRow(`The Cedar app last connected ${when(d.connected_at)}; it isn't running now, so nothing would show.`)];
      return [helpRow('Waiting for the Cedar app to connect.')];
    }

    function browserBlock(local) {
      const parts = [];
      const subs = status.browser?.subscriptions || [];
      if (subs.length) {
        const list = el('div', 'cedar-notify-subs');
        for (const s of subs) {
          const r = el('div', 'cedar-notify-sub');
          r.append(el('span', '', `${agentName(s.user_agent)} · since ${when(s.created_at)}${s.last_used_at ? ` · last sent ${when(s.last_used_at)}` : ''}`));
          r.append(button('Remove', async () => {
            try { status = await deps.api('DELETE', `/notifications/push/subscriptions/${s.id}`); } catch (e) { deps.toast(e.message); }
            renderSettings();
          }, 'settings-ghost'));
          list.append(r);
        }
        parts.push(list);
      }
      const fields = el('div', 'cedar-set-fields');
      const contact = input('email', status.browser?.contact, 'you@example.com', 'Contact for the push services');
      contact.addEventListener('input', () => { draft.contact = contact.value; dirty(); });
      fields.append(field('Contact for the push services (optional)', contact));
      parts.push(fields);
      const buttons = [];
      if (!pushSupported()) {
        parts.push(helpRow('This browser cannot take push notifications here: it needs https, or localhost.'));
        return parts;
      }
      if (local) {
        buttons.push(button('Turn off here', async () => {
          try { await turnOffPush(); deps.toast('Push notifications are off in this browser'); } catch (e) { deps.toast(e.message); }
          renderSettings();
        }, 'settings-ghost'));
      } else {
        buttons.push(button('Turn on in this browser', async () => {
          try { await turnOnPush(); deps.toast('Push notifications are on in this browser'); } catch (e) { deps.toast(e.message); }
          renderSettings();
        }, 'settings-ghost'));
      }
      if (subs.length) buttons.push(testButton('browser'));
      parts.push(helpRow(`${local ? 'On in this browser.' : 'Not on in this browser yet.'} The contact goes with every push as the address to reach the operator of this server; some push services want one.`, ...buttons));
      return parts;
    }

    function iosBlock() {
      const a = status.ios || {};
      const parts = [];
      const devices = a.devices || [];
      if (devices.length) {
        const list = el('div', 'cedar-notify-subs');
        for (const d of devices) {
          const row = el('div', 'cedar-notify-sub');
          row.append(el('span', '', `${d.name || 'iPhone'} · since ${when(d.created_at)}${d.last_used_at ? ` · last sent ${when(d.last_used_at)}` : ''}`));
          row.append(button('Remove', async () => {
            try { status = await deps.api('DELETE', `/notifications/apns/devices/${d.id}`); } catch (e) { deps.toast(e.message); }
            renderSettings();
          }, 'settings-ghost'));
          list.append(row);
        }
        parts.push(list);
      }
      const fields = el('div', 'cedar-set-fields');
      const team = input('text', a.team_id, 'ABCDE12345', 'Apple team id');
      team.addEventListener('input', () => { draft.ios.team_id = team.value; dirty(); });
      const keyId = input('text', a.key_id, 'ABC123DEFG', 'APNs key id');
      keyId.addEventListener('input', () => { draft.ios.key_id = keyId.value; dirty(); });
      const topic = input('text', a.topic, 'dev.cedar.Cedar', 'Bundle id');
      topic.addEventListener('input', () => { draft.ios.topic = topic.value; dirty(); });
      const env = el('select');
      env.setAttribute('aria-label', 'APNs environment');
      env.append(new Option('Production (App Store, TestFlight)', 'production', false, a.environment !== 'sandbox'));
      env.append(new Option('Sandbox (installed from Xcode)', 'sandbox', false, a.environment === 'sandbox'));
      env.addEventListener('change', () => { draft.ios.environment = env.value; dirty(); });
      const key = el('textarea');
      key.rows = 4;
      key.placeholder = a.key_set ? 'Saved; paste to replace' : '-----BEGIN PRIVATE KEY-----\n…\n-----END PRIVATE KEY-----';
      key.setAttribute('aria-label', 'APNs key (.p8 file)');
      key.spellcheck = false;
      key.addEventListener('input', () => { draft.ios.private_key = key.value; dirty(); });
      fields.append(field('Team id', team), field('Key id', keyId), field('Bundle id', topic), field('Environment', env));
      const keyField = field('APNs key (the .p8 file\'s contents)', key);
      keyField.style.gridColumn = '1 / -1';
      fields.append(keyField);
      parts.push(fields);
      const buttons = [];
      if (devices.length) buttons.push(testButton('ios'));
      if (a.key_set) {
        buttons.push(button('Forget the saved key', async () => {
          try { await submit({ ios: { team_id: '', key_id: '', private_key: '' }, default_method: draft.default_method === 'ios' ? 'none' : draft.default_method }); } catch { /* reported */ }
          renderSettings();
        }, 'settings-ghost'));
      }
      const registered = devices.length
        ? `${devices.length} ${devices.length === 1 ? 'iPhone has' : 'iPhones have'} registered.`
        : 'No iPhone has registered yet: open the Cedar app, point it at this server, and allow notifications.';
      parts.push(helpRow(`${registered} In the Apple developer account, under Certificates, Identifiers & Profiles → Keys, make a key with Apple Push Notifications service (APNs) on, download its .p8 file once, and note the key id; the team id is under Membership. The key stays on this server.`, ...buttons));
      return parts;
    }

    function pushoverBlock() {
      const p = status.pushover || {};
      const fields = el('div', 'cedar-set-fields');
      const token = input('password', '', p.token_set ? 'Saved; type to replace' : 'Application token from pushover.net', 'Pushover application token');
      token.addEventListener('input', () => { draft.pushover.token = token.value; dirty(); });
      const user = input('password', '', p.user_set ? `Saved (…${p.user_last4}); type to replace` : 'Your user key', 'Pushover user key');
      user.addEventListener('input', () => { draft.pushover.user = user.value; dirty(); });
      fields.append(field('Application token', token), field('User key', user));
      const buttons = [testButton('pushover')];
      if (p.token_set || p.user_set) {
        buttons.push(button('Forget the saved keys', async () => {
          try { await submit({ pushover: { token: '', user: '' }, default_method: draft.default_method === 'pushover' ? 'none' : draft.default_method }); } catch { /* reported */ }
          renderSettings();
        }, 'settings-ghost'));
      }
      return [fields, helpRow('Make an application at pushover.net/apps for its token; your user key is on the pushover.net front page. Both stay on this server.', ...buttons)];
    }

    function emailBlock() {
      const e = status.email || {};
      const fields = el('div', 'cedar-set-fields');
      const host = input('text', e.host, 'smtp.example.com', 'SMTP server');
      host.addEventListener('input', () => { draft.email.host = host.value; dirty(); });
      const security = el('select');
      security.setAttribute('aria-label', 'Connection');
      for (const [id, label] of [['tls', 'TLS (port 465)'], ['starttls', 'STARTTLS (port 587)'], ['none', 'Plain (a relay on this machine)']]) {
        security.append(new Option(label, id, false, (e.security || 'tls') === id));
      }
      const port = input('number', e.port ?? '', 'Default for the connection', 'Port');
      port.min = '1';
      port.max = '65535';
      security.addEventListener('change', () => { draft.email.security = security.value; dirty(); });
      port.addEventListener('input', () => { draft.email.port = port.value ? Number(port.value) : 0; dirty(); });
      const username = input('text', e.username, 'Often the full address', 'SMTP user name');
      username.addEventListener('input', () => { draft.email.username = username.value; dirty(); });
      const password = input('password', '', e.password_set ? 'Saved; type to replace' : 'Password or app password', 'SMTP password');
      password.addEventListener('input', () => { draft.email.password = password.value; dirty(); });
      const from = input('email', e.from, 'cedar@example.com', 'Sender');
      from.addEventListener('input', () => { draft.email.from = from.value; dirty(); });
      const to = input('email', e.to, 'you@example.com', 'Recipient');
      to.addEventListener('input', () => { draft.email.to = to.value; dirty(); });
      fields.append(field('SMTP server', host), field('Connection', security), field('Port', port), field('User name', username), field('Password', password), field('From', from), field('To', to));
      const buttons = [testButton('email')];
      if (e.password_set) {
        buttons.push(button('Forget the saved password', async () => {
          try { await submit({ email: { password: '' } }); } catch { /* reported */ }
          renderSettings();
        }, 'settings-ghost'));
      }
      return [fields, helpRow('The password stays on this server. A Gmail account takes an app password with STARTTLS on smtp.gmail.com.', ...buttons)];
    }
  }

  /** PUT the draft; `extra` goes along (a test's or a clear's fields). Resolves to the new status. */
  async function submit(extra = {}) {
    if (!draft) return status;
    const body = { default_method: draft.default_method, scope: draft.scope, ...extra };
    if (draft.contact !== undefined) body.contact = draft.contact;
    if (Object.keys(draft.pushover).length) body.pushover = draft.pushover;
    if (Object.keys(draft.email).length) body.email = draft.email;
    if (Object.keys(draft.ios).length) body.ios = draft.ios;
    try {
      status = await deps.api('PUT', '/notifications', body);
      draft = { default_method: status.default_method, scope: status.scope, pushover: {}, email: {}, ios: {} };
      updateAwayLine();
      const s = shell();
      s?.setDirty('notify', 0);
      s?.refresh('notify');
      guard();
      return status;
    } catch (e) {
      deps.toast(e.message);
      throw e;
    }
  }

  /** The footer's Save: save, then redraw with what the server kept. */
  async function save() {
    await submit();
    await renderSettings();
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
    section = document.getElementById('notify-section');
    // settings-shell.js opens the Settings dialog and calls render each time it opens; what was
    // picked but not saved is dropped when it closes.
    document.getElementById('settings-dialog')?.addEventListener('close', () => { draft = null; openMethod = null; });
    shell()?.register({
      id: 'notify', label: 'Notifications', title: 'Notifications',
      description: 'When a response finishes while you are away. The inbox and the marks in the list are always on.',
      render: renderSettings, summary, save, footer: true,
    });
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

  return { init, start, setTitle, seen, refresh, loadStatus, methodOptions, methodLabel: (m) => METHOD_LABELS[m] || m, unread: () => inbox.unread };
})();

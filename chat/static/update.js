'use strict';

// The update banner, shared by every UI: a bar at the top of the page when a newer Cedar release
// is out, with its release notes, **Update** and **Dismiss** (for that version only, in this
// browser). It needs nothing from the app: it reads the access token the app saved and calls the
// API itself.
//
//   - Served by a self-hosted cedar-chat (Linux): GET /api/update says what is out. With the
//     root `cedar-update` unit set up (CHAT_UPDATE_DIR), Update asks for the admin token if this
//     tab doesn't have it, POSTs /api/update and follows the install until the server comes back
//     on the new version, then reloads. Without it, Update shows the commands to run.
//   - Inside the Cedar desktop app (macOS, Windows), whose bundled cedar-chat doesn't check: the
//     app pushes its own updater's state in with cedarUpdate.fromApp(state), and Update and
//     Dismiss send the page to /__cedar-app/update/install or …/dismiss, which the app's window
//     catches instead of loading (see desktop/src/window.rs). The page gets no other way to
//     call the app.

const cedarUpdate = (() => {
  const TOKEN_KEY = 'cedar-chat-token';
  const ADMIN_KEY = 'cedar-chat-admin';
  const DISMISS_KEY = 'cedar-update-dismissed';
  const POLL = 30 * 60 * 1000;
  const POLL_INSTALLING = 3000;
  const STEPS = {
    requested: 'Waiting for the updater…',
    downloading: 'Downloading…',
    verifying: 'Checking the download…',
    installing: 'Installing…',
    restarting: 'Restarting Cedar…',
  };
  const inDesktopApp = window.__CEDAR_DESKTOP_APP__ === true;

  let bar = null;
  let status = null;       // GET /api/update, or what the app pushed in
  let timer = null;
  let restartingFrom = null;  // the version that was running when the restart began

  function el(tag, className, text) {
    const e = document.createElement(tag);
    if (className) e.className = className;
    if (text !== undefined) e.textContent = text;
    return e;
  }

  function button(label, onClick, className) {
    const b = el('button', className, label);
    b.type = 'button';
    b.addEventListener('click', onClick);
    return b;
  }

  function stored(store, key) {
    try { return store.getItem(key) || ''; } catch { return ''; }
  }

  function remember(store, key, value) {
    try { store.setItem(key, value); } catch { /* lasts until the page is reloaded */ }
  }

  async function api(method, path, admin) {
    const headers = {};
    const token = stored(localStorage, TOKEN_KEY);
    if (token) headers.Authorization = `Bearer ${token}`;
    if (admin) headers['X-Admin-Token'] = admin;
    const resp = await fetch(`/api${path}`, { method, headers });
    const body = await resp.json().catch(() => null);
    if (!resp.ok) {
      const err = new Error(body?.error?.message || `${resp.status}`);
      err.status = resp.status;
      throw err;
    }
    return body;
  }

  function schedule(ms) {
    clearTimeout(timer);
    timer = setTimeout(load, ms);
  }

  async function load() {
    if (inDesktopApp) return;
    try {
      status = await api('GET', '/update');
    } catch (e) {
      // Before the token is entered, or while the server restarts onto the new version.
      schedule(restartingFrom ? POLL_INSTALLING : e.status === 401 ? 5000 : POLL);
      return;
    }
    if (restartingFrom && status.current !== restartingFrom) {
      window.location.reload();
      return;
    }
    const step = status.install?.state;
    if (step === 'restarting' && !restartingFrom) restartingFrom = status.current;
    render();
    schedule(STEPS[step] ? POLL_INSTALLING : POLL);
  }

  // -- the bar -----------------------------------------------------------------------------------

  function ensureBar() {
    if (bar) return bar;
    bar = el('div', 'cedar-update');
    bar.setAttribute('role', 'status');
    bar.hidden = true;
    document.body.append(bar);
    return bar;
  }

  function latestVersion() {
    return status?.latest?.version || status?.version || '';
  }

  function render() {
    const b = ensureBar();
    b.replaceChildren();
    const version = latestVersion();
    const step = status?.install?.state;
    const busy = Boolean(STEPS[step]);
    const failed = step === 'failed';
    const dismissed = inDesktopApp ? status?.dismissed : stored(localStorage, DISMISS_KEY) === version;
    const show = status?.available && version && (busy || failed || !dismissed);
    b.hidden = !show;
    if (!show) return;

    const text = el('span', 'cedar-update-text');
    if (busy) {
      text.append(el('strong', '', `Updating to Cedar ${version}`), ` · ${status.install.detail || STEPS[step] || 'Working…'}`);
      b.append(text);
      return;
    }
    text.append(el('strong', '', `Cedar ${version} is available`), ` · you have ${status.current}`);
    if (failed) text.append(el('span', 'cedar-update-err', ` · The update failed: ${status.install.detail || 'see the logs'}`));
    b.append(text);
    const notes = status.latest?.notes_url || status.notes_url;
    if (notes) {
      const a = el('a', 'cedar-update-link', 'Release notes');
      a.href = notes;
      a.target = '_blank';
      a.rel = 'noopener';
      b.append(a);
    }
    if (inDesktopApp || status.can_install) {
      b.append(button(failed ? 'Try again' : 'Update', update, 'cedar-update-go'));
    } else if (status.commands) {
      b.append(button('How to update', showCommands, 'cedar-update-go'));
    }
    b.append(button('Dismiss', () => {
      if (inDesktopApp) {
        // The app remembers it, for its setup page and the tray too, and pushes the new state.
        window.location.href = '/__cedar-app/update/dismiss';
        return;
      }
      remember(localStorage, DISMISS_KEY, version);
      render();
    }, 'cedar-update-close'));
  }

  // -- updating ----------------------------------------------------------------------------------

  async function confirmUpdate(message) {
    if (typeof cedarConfirm === 'function') return cedarConfirm(message, { ok: 'Update' });
    return window.confirm(message);
  }

  async function update() {
    const version = latestVersion();
    const ok = await confirmUpdate(
      `Update to Cedar ${version}?\n\nCedar restarts to finish, which stops any response that is still running.`
    );
    if (!ok) return;
    if (inDesktopApp) {
      // The app's window catches this address and installs; the page stays where it is.
      window.location.href = '/__cedar-app/update/install';
      return;
    }
    let admin = stored(sessionStorage, ADMIN_KEY);
    if (!admin) {
      admin = await askAdmin();
      if (!admin) return;
    }
    try {
      status = await api('POST', '/update', admin);
      remember(sessionStorage, ADMIN_KEY, admin);
      render();
      schedule(POLL_INSTALLING);
    } catch (e) {
      if (e.status === 403) {
        try { sessionStorage.removeItem(ADMIN_KEY); } catch { /* nothing kept */ }
      }
      showMessage('The update could not start', e.message);
    }
  }

  function dialogShell(title) {
    const dialog = el('dialog', 'cedar-update-dialog');
    dialog.append(el('h3', '', title));
    dialog.addEventListener('close', () => dialog.remove());
    return dialog;
  }

  function actions(dialog, ...buttons) {
    const form = el('form', 'dialog-actions');
    form.method = 'dialog';
    form.style.gap = '8px';
    form.append(...buttons);
    dialog.append(form);
  }

  function dialogButton(label, value, className) {
    const b = el('button', className, label);
    b.value = value;
    return b;
  }

  function showMessage(title, message) {
    const dialog = dialogShell(title);
    dialog.append(el('p', '', message));
    actions(dialog, dialogButton('OK', 'ok', 'primary'));
    document.body.append(dialog);
    dialog.showModal();
  }

  /** The admin token (CHAT_ADMIN_TOKEN), which starting an install needs. */
  function askAdmin() {
    return new Promise((resolve) => {
      const dialog = dialogShell('Admin token');
      dialog.append(el('p', '', 'Installing an update needs the admin token (CHAT_ADMIN_TOKEN in /etc/cedar/chat.env).'));
      const input = el('input', 'cedar-update-input');
      input.type = 'password';
      input.autocomplete = 'off';
      input.placeholder = 'Admin token';
      dialog.append(input);
      actions(dialog, dialogButton('Cancel', 'cancel', 'ghost'), dialogButton('Update', 'ok', 'primary'));
      dialog.addEventListener('close', () => resolve(dialog.returnValue === 'ok' ? input.value.trim() : ''));
      document.body.append(dialog);
      dialog.showModal();
      input.focus();
    });
  }

  function showCommands() {
    const dialog = dialogShell(`Update to Cedar ${latestVersion()}`);
    dialog.append(el('p', '', 'Run these on the server (DEPLOYMENT.md, "Updates", sets up one-click updates):'));
    const pre = el('pre', 'cedar-update-commands');
    pre.textContent = status.commands;
    dialog.append(pre);
    const copy = dialogButton('Copy', 'copy', 'ghost');
    copy.addEventListener('click', (e) => {
      e.preventDefault();
      navigator.clipboard?.writeText(status.commands).then(() => { copy.textContent = 'Copied'; }, () => {});
    });
    actions(dialog, copy, dialogButton('Close', 'ok', 'primary'));
    document.body.append(dialog);
    dialog.showModal();
  }

  // -- the desktop app's side --------------------------------------------------------------------

  /** The app's updater state: {available, version, current, notes_url, dismissed, install: {state, detail}}. */
  function fromApp(state) {
    status = state;
    render();
  }

  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', load);
  else load();

  return { fromApp, refresh: load };
})();

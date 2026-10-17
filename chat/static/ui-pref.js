'use strict';

// Which front end to show: "v1" (Claude style), "v2" (Cedar style), "v3" (Ledger) or "v4"
// (Dossier). Shared by the loader at / and by every UI, each a separate page under /vN/. The choice
// is kept in localStorage and only decides where / goes; visiting /vN/ directly always shows that UI.
// Pages that contain #settings-dialog also get their Settings menu wired up here.

const cedarUI = (() => {
  const KEY = 'cedar-chat-ui';
  const DEFAULT = 'v3';
  const LABELS = { v1: 'Claude style', v2: 'Cedar style', v3: 'Ledger', v4: 'Dossier' };
  const valid = (ui) => typeof ui === 'string' && Object.hasOwn(LABELS, ui);

  function get() {
    try {
      const saved = localStorage.getItem(KEY);
      if (valid(saved)) return saved;
    } catch { /* storage may be unavailable; fall back to the default */ }
    return DEFAULT;
  }

  function set(ui) {
    if (!valid(ui)) return;
    try { localStorage.setItem(KEY, ui); } catch { /* the switch then only lasts for this visit */ }
  }

  /** Open a UI, keeping the #/c/<id> hash so the same conversation stays open, and the query
   *  (the outcome of "Connect Google", which settings.js reports). */
  function go(ui, replace) {
    const url = `/${valid(ui) ? ui : DEFAULT}/${location.search}${location.hash}`;
    if (replace) location.replace(url);
    else location.assign(url);
  }

  function bindSettings() {
    const dialog = document.getElementById('settings-dialog');
    const open = document.getElementById('settings-btn');
    if (!dialog || !open) return;
    const current = document.documentElement.dataset.ui;
    const options = dialog.querySelectorAll('.ui-option[data-ui]');
    for (const option of options) {
      option.setAttribute('aria-pressed', String(option.dataset.ui === current));
      option.addEventListener('click', () => {
        set(option.dataset.ui);
        if (option.dataset.ui !== current) go(option.dataset.ui);
        else dialog.close();
      });
    }
    open.addEventListener('click', () => dialog.showModal());
    document.getElementById('settings-close')?.addEventListener('click', () => dialog.close());
  }

  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', bindSettings);
  else bindSettings();

  return { KEY, DEFAULT, LABELS, get, set, go };
})();

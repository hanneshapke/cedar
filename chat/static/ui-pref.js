'use strict';

// Which front end to show: "v3" (Ledger), "v4" (Dossier), "v2" (Cedar style) or "v1"
// (Claude style). Shared by the loader at / and by every UI, each a separate page under /vN/. The choice
// is kept in localStorage and only decides where / goes; visiting /vN/ directly always shows that UI.
// Pages that contain #settings-dialog also get the Interface page of Settings wired up here; the
// dialog itself is framed by settings-shell.js.

const cedarUI = (() => {
  const KEY = 'cedar-chat-ui';
  const DEFAULT = 'v3';
  const LABELS = { v3: 'Ledger', v4: 'Dossier', v2: 'Cedar style', v1: 'Claude style' };
  /** The UI's name in the page's language (Ledger and Dossier are names and stay as they are). */
  const label = (ui) => (ui === 'v2' ? tr('ui.cedarStyle') : ui === 'v1' ? tr('ui.claudeStyle') : LABELS[ui]);
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
    if (!dialog) return;
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
    if (typeof cedarSettingsShell !== 'undefined') {
      cedarSettingsShell.register({
        id: 'interface', label: tr('ui.title'), title: tr('ui.title'),
        description: tr('ui.description'),
        render: () => {},
        summary: () => label(document.documentElement.dataset.ui) || null,
        footer: false,
      });
    }
  }

  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', bindSettings);
  else bindSettings();

  return { KEY, DEFAULT, LABELS, get, set, go };
})();

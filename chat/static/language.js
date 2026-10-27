'use strict';

// Picking the language: the first thing a new browser asks, before anything else on the page,
// and the Language page of Settings. Both list the locales of cedarI18n.LOCALES; picking one saves
// it in this browser and on the server (scheduled runs and notifications follow it) and reloads
// the page in it. The first visit preselects the language saved on the server by another device,
// else the browser's own.
//
// The UIs wait for `cedarLanguage.ready` before they start, so the token prompt and everything
// else come after the question.

const cedarLanguage = (() => {
  const { LOCALES, locale } = cedarI18n;

  function el(tag, className, text) {
    const e = document.createElement(tag);
    if (className) e.className = className;
    if (text !== undefined) e.textContent = text;
    return e;
  }

  function labelOf(tag) {
    return LOCALES.find((l) => l.tag === tag)?.label
      || LOCALES.find((l) => l.tag === tag.split('-')[0])?.label
      || cedarI18n.LANGUAGES[tag.split('-')[0]]
      || tag;
  }

  /** The locale list as buttons; `current` is marked, `onPick(tag)` is called on a click. */
  function choices(current, onPick, auto) {
    const group = el('div', 'ui-choice lang-choice');
    group.setAttribute('role', 'group');
    group.setAttribute('aria-label', tr('language.title'));
    const options = auto ? [{ tag: '', label: tr('language.auto'), note: tr('language.autoNote') }, ...LOCALES] : LOCALES;
    for (const option of options) {
      const b = el('button', 'ui-option');
      b.type = 'button';
      b.dataset.locale = option.tag;
      b.lang = option.tag || cedarI18n.locale;
      b.setAttribute('aria-pressed', String(option.tag === current));
      b.append(el('strong', '', option.label));
      if (option.note) b.append(el('span', '', option.note));
      b.addEventListener('click', () => onPick(option.tag));
      group.append(b);
    }
    return group;
  }

  // -- the first visit -------------------------------------------------------------------------

  /** The language saved on the server, if the page may read it yet (no token prompt here). */
  async function serverLocale() {
    try {
      const headers = {};
      const token = localStorage.getItem('cedar-chat-token');
      if (token) headers.Authorization = `Bearer ${token}`;
      const res = await fetch('/api/settings', { headers });
      if (!res.ok) return '';
      return cedarI18n.normalize((await res.json())?.locale);
    } catch {
      return '';
    }
  }

  function ask() {
    return new Promise((resolve) => {
      const dialog = el('dialog', 'lang-dialog');
      dialog.setAttribute('aria-labelledby', 'lang-dialog-title');
      const head = el('div', 'lang-dialog-head');
      // Asked in all three languages, since the page can't know which one yet.
      const title = el('h2', 'lang-dialog-title', 'Choose your language');
      title.id = 'lang-dialog-title';
      head.append(
        title,
        el('p', 'lang-dialog-sub', 'Sprache wählen · Choisissez votre langue · Elige tu idioma'),
        el('p', 'lang-dialog-note', tr('language.onboardingNote')),
      );
      let picked = locale;
      const list = choices(picked, (tag) => {
        picked = tag;
        for (const b of list.querySelectorAll('.ui-option')) b.setAttribute('aria-pressed', String(b.dataset.locale === tag));
      });
      const go = el('button', 'lang-dialog-go', tr('language.continue'));
      go.type = 'button';
      go.addEventListener('click', async () => {
        go.disabled = true;
        await cedarI18n.set(picked);
        dialog.close();
        dialog.remove();
        resolve();
      });
      // The question must be answered: Escape doesn't skip it.
      dialog.addEventListener('cancel', (e) => e.preventDefault());
      dialog.append(head, list, go);
      document.body.append(dialog);
      dialog.showModal();
      // Focus starts on Continue, so the first choice doesn't look picked.
      go.focus();
      serverLocale().then((saved) => {
        const option = saved && list.querySelector(`.ui-option[data-locale="${saved}"]`);
        if (option && picked === locale) option.click();
      });
    });
  }

  const ready = (async () => {
    if (document.readyState === 'loading') await new Promise((r) => document.addEventListener('DOMContentLoaded', r, { once: true }));
    if (!cedarI18n.chosen()) await ask();
  })();

  // -- Settings ----------------------------------------------------------------------------------

  function bindSettings() {
    if (typeof cedarSettingsShell === 'undefined') return;
    const body = document.getElementById('settings-body');
    if (!body) return;
    let section = body.querySelector('.settings-page[data-page="language"]');
    if (!section) {
      section = el('section', 'settings-page');
      section.dataset.page = 'language';
      section.id = 'language-section';
      section.hidden = true;
      body.prepend(section);
    }
    cedarSettingsShell.register({
      id: 'language',
      label: tr('language.title'),
      title: tr('language.title'),
      description: tr('language.description'),
      render(sectionEl) {
        sectionEl.textContent = '';
        const current = cedarI18n.chosen() ? cedarI18n.locale : '';
        sectionEl.append(choices(current, (tag) => cedarI18n.set(tag), true));
      },
      summary: () => labelOf(cedarI18n.locale),
      footer: false,
    });
  }

  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', bindSettings);
  else bindSettings();

  return { ready, labelOf };
})();

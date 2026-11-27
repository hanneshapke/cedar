'use strict';

// The Language page of Settings. It lists the locales of cedarI18n.LOCALES; picking one saves it
// in this browser and on the server (scheduled runs and notifications follow it) and reloads the
// page in it.
//
// Nobody is asked for a language when they first open the page: the browser already says which
// one its owner reads (i18n.js starts in it, or in the one another device saved on the server),
// and this page changes it.

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

  /** Kept for the page's start-up, which once waited here for the first visit's question. */
  const ready = Promise.resolve();

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

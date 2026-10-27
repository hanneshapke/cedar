'use strict';

// The setup page's text in the user's language: English, German or French. A small copy of the
// chat's runtime (chat/static/i18n.js), without the chat server: the language is saved in the
// app's config.json (the `set_locale` command), so the tray menu and the app's own messages use it
// too. Loaded first, before the catalog (strings.js) that fills it:
//
//   cedarI18n.add({ en: { 'key': 'Text with {name}' }, de: { ... }, fr: { ... } });
//
// tr('key', { name }) (cedarI18n.t) looks a key up in the current language, then in English,
// and falls back to the key itself. A value may be an object of plural forms ({ one, other })
// picked by params.count. Elements carry their text as data-i18n="key" (textContent),
// data-i18n-html="key" (markup from the catalog itself, for text with links in it) and
// data-i18n-attr="placeholder:key; title:key"; apply(root) fills them in. Swiss German (de-CH)
// writes ss for ß.

const cedarI18n = (() => {
  /** The languages the page is translated into, each named in its own language. */
  const LANGUAGES = { en: 'English', de: 'Deutsch', fr: 'Français' };
  /** The locales offered, in order, with their own names. */
  const LOCALES = [
    { tag: 'en', label: 'English' },
    { tag: 'en-US', label: 'English (United States)' },
    { tag: 'en-GB', label: 'English (United Kingdom)' },
    { tag: 'de-DE', label: 'Deutsch (Deutschland)' },
    { tag: 'de-AT', label: 'Deutsch (Österreich)' },
    { tag: 'de-CH', label: 'Deutsch (Schweiz)' },
    { tag: 'fr-FR', label: 'Français (France)' },
    { tag: 'fr-BE', label: 'Français (Belgique)' },
    { tag: 'fr-CH', label: 'Français (Suisse)' },
    { tag: 'fr-CA', label: 'Français (Canada)' },
  ];
  const catalogs = { en: {}, de: {}, fr: {} };

  let locale = 'en';
  let lang = 'en';
  let swiss = false;
  let plurals = null;

  /** `raw` as one of LOCALES' tags, or '' if it isn't one. */
  function normalize(raw) {
    const want = String(raw || '').trim().replace(/_/g, '-').toLowerCase();
    const hit = LOCALES.find((l) => l.tag.toLowerCase() === want);
    return hit ? hit.tag : '';
  }

  /** The offered locale closest to the system's languages (`de-LU` → `de-DE`), or `en`. */
  function suggested() {
    const wanted = navigator.languages?.length ? navigator.languages : [navigator.language];
    for (const tag of wanted) {
      const exact = normalize(tag);
      if (exact) return exact;
      const l = String(tag || '').split(/[-_]/)[0].toLowerCase();
      if (l !== 'en' && Object.hasOwn(LANGUAGES, l)) return LOCALES.find((x) => x.tag.startsWith(l + '-')).tag;
      if (l === 'en') return 'en';
    }
    return 'en';
  }

  /** Use `tag` from now on (anything not offered: English). */
  function use(tag) {
    locale = normalize(tag) || 'en';
    lang = locale.split('-')[0];
    swiss = locale === 'de-CH';
    plurals = null;
    document.documentElement.lang = locale;
  }

  function add(byLanguage) {
    for (const [l, entries] of Object.entries(byLanguage || {})) {
      if (catalogs[l]) Object.assign(catalogs[l], entries);
    }
  }

  function pick(value, count) {
    if (value === null || typeof value !== 'object') return value;
    plurals ||= new Intl.PluralRules(locale);
    const form = typeof count === 'number' ? plurals.select(count) : 'other';
    return value[form] ?? value.other ?? Object.values(value)[0];
  }

  function number(n, opts) {
    return Number(n).toLocaleString(locale, opts);
  }

  /** The text for `key` in the user's language, with {name} placeholders filled from params. */
  function t(key, params) {
    let value = catalogs[lang][key];
    if (value === undefined) value = catalogs.en[key];
    if (value === undefined) value = key;
    let text = String(pick(value, params?.count));
    if (params) {
      text = text.replace(/\{(\w+)\}/g, (m, name) => {
        if (!Object.hasOwn(params, name)) return m;
        const v = params[name];
        return typeof v === 'number' ? number(v) : String(v);
      });
    }
    return swiss ? text.replace(/ß/g, 'ss') : text;
  }

  /** Fill in data-i18n, data-i18n-html and data-i18n-attr under `root`. */
  function apply(root) {
    const scope = root || document;
    for (const node of scope.querySelectorAll('[data-i18n]')) node.textContent = t(node.dataset.i18n);
    // Only the catalog's own markup goes in here, never text from elsewhere.
    for (const node of scope.querySelectorAll('[data-i18n-html]')) node.innerHTML = t(node.dataset.i18nHtml);
    for (const node of scope.querySelectorAll('[data-i18n-attr]')) {
      for (const pair of node.dataset.i18nAttr.split(';')) {
        const [attr, key] = pair.split(':').map((s) => s.trim());
        if (attr && key) node.setAttribute(attr, t(key));
      }
    }
  }

  return {
    LANGUAGES, LOCALES, add, t, number, apply, use, normalize, suggested,
    get locale() { return locale; },
    get lang() { return lang; },
  };
})();

/** Shorthand for the page's script. */
const tr = cedarI18n.t;

'use strict';

// The pages' text in the user's language: English, German, French or Spanish, with the region
// (de-CH, fr-CA, es-MX, ...) deciding how dates, numbers and prices are written. Loaded first by
// every page, before the catalogs in /i18n/ that fill it:
//
//   cedarI18n.add({ en: { 'key': 'Text with {name}' }, de: { ... }, fr: { ... }, es: { ... },
//                   'es-ES': { 'key': 'only what Spain says differently' } });
//
// tr('key', { name }) (cedarI18n.t) looks a key up in the user's region's catalog, then their
// language's, then English, and falls back to the key itself. A value may be an object of plural forms ({ one, other })
// picked by params.count. Elements carry their text as data-i18n="key" (textContent) and
// data-i18n-attr="placeholder:key; title:key", which apply(root) fills in; each page calls
// apply(document) once its catalogs are loaded.
//
// The language is chosen per browser (localStorage) and saved on the server too, so scheduled
// runs and notifications use it and a new browser starts in it. Until someone picks one, the
// browser's own language is used if it is one of ours. Swiss German (de-CH) writes ss for ß.

const cedarI18n = (() => {
  const KEY = 'cedar-locale';
  /** The languages the pages are translated into, each named in its own language. */
  const LANGUAGES = { en: 'English', de: 'Deutsch', fr: 'Français', es: 'Español' };
  /** The locales offered in Settings and at the first visit, in order, with their own names. */
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
    { tag: 'es-ES', label: 'Español (España)' },
    { tag: 'es-MX', label: 'Español (México)' },
    { tag: 'es-419', label: 'Español (Latinoamérica)' },
    { tag: 'es-US', label: 'Español (Estados Unidos)' },
  ];
  // By language, plus regional catalogs ('es-ES') that hold only the text that differs there.
  const catalogs = { en: {}, de: {}, fr: {}, es: {} };

  /** `raw` as `de` or `de-CH`, or '' if it isn't a tag in one of our languages. */
  function normalize(raw) {
    const parts = String(raw || '').trim().split(/[-_]/);
    const lang = (parts[0] || '').toLowerCase();
    if (!Object.hasOwn(LANGUAGES, lang)) return '';
    const region = parts.slice(1).find((p) => /^([A-Za-z]{2}|\d{3})$/.test(p));
    return region ? `${lang}-${region.toUpperCase()}` : lang;
  }

  function stored() {
    try { return normalize(localStorage.getItem(KEY)); } catch { return ''; }
  }

  /** The browser's first language that is one of ours, with its region. */
  function fromBrowser() {
    const wanted = navigator.languages?.length ? navigator.languages : [navigator.language];
    for (const tag of wanted) {
      const n = normalize(tag);
      if (n) return n;
    }
    return 'en';
  }

  const locale = stored() || fromBrowser();
  const lang = locale.split('-')[0];
  const swiss = locale === 'de-CH';
  document.documentElement.lang = locale;

  function add(byLanguage) {
    for (const [l, entries] of Object.entries(byLanguage || {})) {
      if (catalogs[l]) Object.assign(catalogs[l], entries);
      else if (l.includes('-') && normalize(l) === l) Object.assign((catalogs[l] ||= {}), entries);
    }
  }

  let plurals = null;
  function pick(value, count) {
    if (value === null || typeof value !== 'object') return value;
    plurals ||= new Intl.PluralRules(locale);
    const form = typeof count === 'number' ? plurals.select(count) : 'other';
    return value[form] ?? value.other ?? Object.values(value)[0];
  }

  /** The text for `key` in the user's language, with {name} placeholders filled from params. */
  function t(key, params) {
    let value = catalogs[locale]?.[key];
    if (value === undefined) value = catalogs[lang][key];
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

  /** Whether the catalogs have `key` (in any language). */
  function has(key) {
    return catalogs[locale]?.[key] !== undefined || catalogs[lang][key] !== undefined || catalogs.en[key] !== undefined;
  }

  function number(n, opts) {
    return Number(n).toLocaleString(locale, opts);
  }

  /** Fill in data-i18n and data-i18n-attr under `root`, templates included. */
  function apply(root) {
    const scope = root || document;
    for (const node of scope.querySelectorAll('[data-i18n]')) node.textContent = t(node.dataset.i18n);
    for (const node of scope.querySelectorAll('[data-i18n-attr]')) {
      for (const pair of node.dataset.i18nAttr.split(';')) {
        const [attr, key] = pair.split(':').map((s) => s.trim());
        if (attr && key) node.setAttribute(attr, t(key));
      }
    }
    for (const tpl of scope.querySelectorAll('template')) apply(tpl.content);
  }

  function token() {
    try { return localStorage.getItem('cedar-chat-token') || ''; } catch { return ''; }
  }

  /** Save the language on the server, for scheduled runs, notifications and new browsers. */
  async function save(tag) {
    const headers = { 'Content-Type': 'application/json' };
    const tok = token();
    if (tok) headers.Authorization = `Bearer ${tok}`;
    const res = await fetch('/api/settings/locale', { method: 'PUT', headers, body: JSON.stringify({ locale: tag || null }) });
    if (!res.ok) {
      const body = await res.json().catch(() => null);
      throw new Error(body?.error?.message || `${res.status}`);
    }
  }

  const PENDING = 'cedar-locale-pending';

  /** Switch to `tag` (or back to the browser's language with ''): saved here and on the server,
   *  then the page reloads in it. If the server can't be told yet (no access token on a first
   *  visit), it is told on a later page load. */
  async function set(tag) {
    const n = normalize(tag);
    try {
      if (n) localStorage.setItem(KEY, n);
      else localStorage.removeItem(KEY);
    } catch { /* then only the server remembers it */ }
    try {
      await save(n);
      try { localStorage.removeItem(PENDING); } catch { /* nothing to retry then */ }
    } catch {
      try { localStorage.setItem(PENDING, '1'); } catch { /* the server keeps its language */ }
    }
    if ((n || fromBrowser()) !== locale) location.reload();
  }

  try {
    if (localStorage.getItem(PENDING) && token()) {
      save(stored()).then(() => localStorage.removeItem(PENDING)).catch(() => {});
    }
  } catch { /* storage unavailable: nothing pending */ }

  /** Whether this browser has a language of its own yet (else the first visit asks). */
  function chosen() {
    return Boolean(stored());
  }

  /** Adopt the language saved on the server when this browser hasn't picked one. Returns true
   *  when the page is reloading into it. */
  function adopt(serverLocale) {
    const n = normalize(serverLocale);
    if (!n || chosen()) return false;
    try { localStorage.setItem(KEY, n); } catch { return false; }
    if (n === locale) return false;
    location.reload();
    return true;
  }

  return { KEY, LANGUAGES, LOCALES, locale, lang, add, t, has, number, apply, set, chosen, adopt, normalize };
})();

/** Shorthand for the pages' scripts (`t` is the UIs' element helper). */
const tr = cedarI18n.t;

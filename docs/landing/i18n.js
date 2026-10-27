'use strict';

// The website in English, German or French. Each page loads this first, then its catalogs from
// i18n/ (cedarSite.add({ en: {...}, de: {...}, fr: {...} })), in <head>, before anything else.
//
// The language comes from ?lang=de in the address (and is remembered), else from what the visitor
// picked before, else from the browser's languages; English otherwise. Once the page is parsed,
// apply() fills in the marked-up text:
//
//   data-i18n="key"                      the element's text
//   data-i18n-html="key"                 its HTML, for sentences with links or <code> (catalog text only)
//   data-i18n-attr="title:key; alt:key"  attributes
//   data-lang-switch                     filled with links to the three languages
//
// This runs before support.js boots index.html's template, so the template is read translated.
// support.js would otherwise fetch the page's source again and swap the English template back
// in, which index.html prevents by setting window.__resources before loading it. Text written
// by the pages' scripts goes through tr('key', { name }). Terms and tone follow
// docs/i18n-glossary.md: German says du, French vous.

const cedarSite = (() => {
  const KEY = 'cedar-site-lang';
  const NAMES = { en: 'English', de: 'Deutsch', fr: 'Français' };
  const catalogs = { en: {}, de: {}, fr: {} };

  const pick = (raw) => {
    const lang = String(raw || '').trim().toLowerCase().split(/[-_]/)[0];
    return Object.hasOwn(NAMES, lang) ? lang : '';
  };

  function choose() {
    const asked = pick(new URLSearchParams(location.search).get('lang'));
    if (asked) {
      try { localStorage.setItem(KEY, asked); } catch { /* then only this address says it */ }
      return asked;
    }
    try {
      const saved = pick(localStorage.getItem(KEY));
      if (saved) return saved;
    } catch { /* storage unavailable: fall through to the browser */ }
    for (const tag of navigator.languages?.length ? navigator.languages : [navigator.language]) {
      const lang = pick(tag);
      if (lang) return lang;
    }
    return 'en';
  }

  const lang = choose();
  // The browser's own region, when it speaks this language, for dates and numbers.
  const locale = (navigator.languages || []).find((t) => pick(t) === lang) || lang;
  document.documentElement.lang = lang;

  function add(byLanguage) {
    for (const [l, entries] of Object.entries(byLanguage || {})) {
      if (catalogs[l]) Object.assign(catalogs[l], entries);
    }
  }

  let plurals = null;
  /** The text for `key`, with {name} placeholders filled from params; English when missing. */
  function t(key, params) {
    let value = catalogs[lang][key] ?? catalogs.en[key] ?? key;
    if (value && typeof value === 'object') {
      plurals ||= new Intl.PluralRules(locale);
      value = value[typeof params?.count === 'number' ? plurals.select(params.count) : 'other'] ?? value.other;
    }
    let text = String(value);
    if (params) {
      text = text.replace(/\{(\w+)\}/g, (m, name) => {
        if (!Object.hasOwn(params, name)) return m;
        const v = params[name];
        return typeof v === 'number' ? v.toLocaleString(locale) : String(v);
      });
    }
    return text;
  }

  /** Links to the three languages, the current one marked. */
  function switcher(node) {
    node.textContent = '';
    node.setAttribute('aria-label', t('site.language'));
    Object.entries(NAMES).forEach(([code, name], i) => {
      if (i) node.append(' · ');
      const url = new URL(location.href);
      url.searchParams.set('lang', code);
      const a = document.createElement('a');
      a.href = url.search + url.hash;
      a.lang = code;
      a.hreflang = code;
      a.textContent = name;
      if (code === lang) {
        a.setAttribute('aria-current', 'true');
        a.style.fontWeight = '700';
      }
      node.append(a);
    });
  }

  function apply(root) {
    const scope = root || document;
    for (const node of scope.querySelectorAll('[data-i18n]')) node.textContent = t(node.dataset.i18n);
    for (const node of scope.querySelectorAll('[data-i18n-html]')) node.innerHTML = t(node.dataset.i18nHtml);
    for (const node of scope.querySelectorAll('[data-i18n-attr]')) {
      for (const pair of node.dataset.i18nAttr.split(';')) {
        const [attr, key] = pair.split(':').map((s) => s.trim());
        if (attr && key) node.setAttribute(attr, t(key));
      }
    }
    for (const node of scope.querySelectorAll('[data-lang-switch]')) switcher(node);
    const title = catalogs[lang]['page.title'];
    if (title) document.title = title;
  }

  // Registered before support.js registers its own, so index.html's template is translated
  // before it is read.
  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', () => apply(document));
  else apply(document);

  return { lang, locale, NAMES, add, t, apply };
})();

/** Shorthand for the pages' scripts. */
const tr = cedarSite.t;

#!/usr/bin/env node
// Check the translations of the web chat (chat/static/i18n/*.js), the desktop app
// (desktop/ui/strings.js) and the website (docs/landing/i18n/*.js): every key is in English,
// German and French, the {placeholders} and plural forms match, and every key the pages use exists.
//
//   node scripts/check-i18n.js
//
// CI runs it. The phone apps are checked by their own builds (Xcode and Android lint).
'use strict';

const fs = require('fs');
const path = require('path');
const vm = require('vm');

const root = path.resolve(__dirname, '..');
const LANGS = ['en', 'de', 'fr'];
const problems = [];

/** Load catalog files the way the pages do, with a stub cedarI18n.add (cedarSite.add on the website). */
function loadCatalogs(files) {
  const merged = { en: {}, de: {}, fr: {} };
  const origin = {};
  for (const file of files) {
    const added = [];
    const stub = { add: (c) => added.push(c) };
    const context = { cedarI18n: stub, cedarSite: stub };
    vm.runInNewContext(fs.readFileSync(file, 'utf8'), context, { filename: file });
    for (const catalog of added) {
      for (const lang of Object.keys(catalog)) {
        if (!LANGS.includes(lang)) problems.push(`${rel(file)}: unknown language ${lang}`);
        for (const [key, value] of Object.entries(catalog[lang] || {})) {
          if (lang === 'en' && origin[key] && origin[key] !== file) {
            problems.push(`${rel(file)}: ${key} is also defined in ${rel(origin[key])}`);
          }
          if (lang === 'en') origin[key] = file;
          (merged[lang] ||= {})[key] = value;
        }
      }
    }
  }
  return { merged, origin };
}

const rel = (f) => path.relative(root, f);
const placeholders = (s) => [...String(s).matchAll(/\{(\w+)\}/g)].map((m) => m[1]).sort().join(',');
const forms = (v) => (v && typeof v === 'object' ? Object.values(v) : [v]);

function checkCatalogs({ merged, origin }) {
  const keys = new Set(LANGS.flatMap((l) => Object.keys(merged[l])));
  for (const key of keys) {
    const where = origin[key] ? rel(origin[key]) : '?';
    for (const lang of LANGS) {
      if (merged[lang][key] === undefined) problems.push(`${where}: ${key} has no ${lang} text`);
    }
    const en = merged.en[key];
    if (en === undefined) continue;
    const want = placeholders(forms(en).join(' '));
    for (const lang of LANGS) {
      const value = merged[lang][key];
      if (value === undefined) continue;
      if (typeof value === 'object') {
        for (const form of ['one', 'other']) {
          if (typeof value[form] !== 'string') problems.push(`${where}: ${key} (${lang}) needs the plural form "${form}"`);
        }
      } else if (typeof value !== 'string' || !value.trim()) {
        problems.push(`${where}: ${key} (${lang}) is empty`);
      }
      // Each form may leave out {count}, but none may bring a placeholder English doesn't have.
      for (const form of forms(value)) {
        const extra = placeholders(form).split(',').filter((p) => p && !want.split(',').includes(p));
        if (extra.length) problems.push(`${where}: ${key} (${lang}) has {${extra.join('}, {')}} that English doesn't`);
      }
      if (typeof en !== 'object' && placeholders(value) !== want) {
        problems.push(`${where}: ${key} (${lang}) has placeholders {${placeholders(value)}}, English {${want}}`);
      }
    }
  }
}

/** Keys the sources use: tr('key') / cedarI18n.t('key') and data-i18n attributes. */
function checkUses(sources, en) {
  for (const file of sources) {
    const text = fs.readFileSync(file, 'utf8');
    const used = [
      ...[...text.matchAll(/\b(?:tr|cedarI18n\.t)\(\s*'([^'$]+)'/g)].map((m) => m[1]),
      ...[...text.matchAll(/\b(?:tr|cedarI18n\.t)\(\s*"([^"$]+)"/g)].map((m) => m[1]),
      ...[...text.matchAll(/data-i18n(?:-html)?="([^"]+)"/g)].map((m) => m[1]),
      ...[...text.matchAll(/data-i18n-attr="([^"]+)"/g)].flatMap((m) => m[1].split(';').map((p) => (p.split(':')[1] || '').trim())),
    // A key that ends in '.' is a prefix the code completes at run time.
    ].filter((k) => k && !k.endsWith('.'));
    for (const key of new Set(used)) {
      if (en[key] === undefined) problems.push(`${rel(file)}: uses ${key}, which no catalog defines`);
    }
  }
}

function walk(dir, pick) {
  return fs.readdirSync(dir, { withFileTypes: true }).flatMap((e) => {
    const p = path.join(dir, e.name);
    if (e.isDirectory()) return e.name === 'vendor' || e.name === 'i18n' ? [] : walk(p, pick);
    return pick(p) ? [p] : [];
  });
}

const chatStatic = path.join(root, 'chat', 'static');
const chatCatalogs = fs.readdirSync(path.join(chatStatic, 'i18n')).filter((f) => f.endsWith('.js')).map((f) => path.join(chatStatic, 'i18n', f));
const chat = loadCatalogs(chatCatalogs);
checkCatalogs(chat);
// i18n.js itself only names keys in its comments.
checkUses(walk(chatStatic, (p) => /\.(js|html)$/.test(p) && path.basename(p) !== 'i18n.js'), chat.merged.en);

// The desktop app's setup page: its own catalog, in the same format.
const desktopUi = path.join(root, 'desktop', 'ui');
const desktop = loadCatalogs([path.join(desktopUi, 'strings.js')]);
checkCatalogs(desktop);
checkUses(walk(desktopUi, (p) => /\.(js|html)$/.test(p) && !['i18n.js', 'strings.js'].includes(path.basename(p))), desktop.merged.en);

// The website (docs/landing): each page loads i18n/common.js and its own i18n/<page>.js, so each
// is checked on its own (they may each define page.title).
const landing = path.join(root, 'docs', 'landing');
let siteKeys = 0;
for (const page of fs.readdirSync(landing).filter((f) => f.endsWith('.html'))) {
  const own = path.join(landing, 'i18n', page.replace(/\.html$/, '.js'));
  const site = loadCatalogs([path.join(landing, 'i18n', 'common.js'), ...(fs.existsSync(own) ? [own] : [])]);
  checkCatalogs(site);
  checkUses([path.join(landing, page)], site.merged.en);
  siteKeys += Object.keys(site.merged.en).length;
}

if (problems.length) {
  console.error(problems.join('\n'));
  console.error(`\n${problems.length} problem(s) in the translations`);
  process.exit(1);
}
console.log(`translations ok: ${Object.keys(chat.merged.en).length} web chat keys in ${chatCatalogs.length} catalogs, ${Object.keys(desktop.merged.en).length} desktop keys, ${siteKeys} website keys`);

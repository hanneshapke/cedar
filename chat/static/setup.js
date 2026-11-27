'use strict';

// Setting Cedar up, the first time it is opened. Cedar can't answer until it has a model to
// think with, so instead of a message box that can't send and a pointer to Settings, the page
// starts here and ends on a first task that runs:
//
//   1. look    cedar is asked whether a model server already runs on its computer (Ollama, LM
//              Studio and the others in /model-servers.json). Nobody types an address.
//   2. found   the server it found, with the model it would use picked; one click takes it.
//      choose  nothing found, or "use something else": let the desktop app download a model,
//              paste a key from a hosted provider, or give the address of a server elsewhere.
//   3. ready   three first tasks that need nothing but the sandbox. One click sends one.
//
// The work is settings.js's (`cedarSettings.setup`): this file only walks through it. Someone who
// may not add models here (CHAT_MODELS_ACCESS=admins, without the admin password) is told who can.
//
//   cedarSetup.init({ toast, config, run })   run(prompt) sends a first task in a new conversation
//   cedarSetup.open()                         show it (the page does when no model is registered)

const cedarSetup = (() => {
  let deps = null;
  let root = null;
  let body = null;
  /** The model servers found on cedar's computer: [{ name, baseUrl, models }]. */
  let found = [];
  /** How long one server may take to answer before it counts as not there. */
  const LOOK_MS = 6000;
  /** The first tasks: each needs only the sandbox (no key, no file, no account). */
  const TASKS = ['savings', 'dinners', 'split'];

  const api = () => cedarSettings.setup;

  function el(tag, className, text) {
    const e = document.createElement(tag);
    if (className) e.className = className;
    if (text !== undefined) e.textContent = text;
    return e;
  }

  function button(label, onClick, className = 'cedar-setup-btn') {
    const b = el('button', className, label);
    b.type = 'button';
    b.addEventListener('click', onClick);
    return b;
  }

  /** Replace the step on show, and put the focus on its heading so a screen reader says it. */
  function show(title, lead, ...children) {
    const h = el('h1', 'cedar-setup-title', title);
    h.tabIndex = -1;
    body.replaceChildren(h, ...(lead ? [el('p', 'cedar-setup-lead', lead)] : []), ...children.filter(Boolean));
    h.focus();
  }

  function field(label, input) {
    const f = el('label', 'cedar-setup-field');
    f.append(el('span', '', label), input);
    return f;
  }

  function input(type, placeholder) {
    const i = el('input');
    i.type = type;
    i.placeholder = placeholder || '';
    i.spellcheck = false;
    i.autocapitalize = 'off';
    i.autocomplete = 'off';
    return i;
  }

  const errorLine = () => {
    const p = el('p', 'cedar-setup-error');
    p.setAttribute('role', 'alert');
    p.hidden = true;
    return p;
  };
  const fail = (line, message) => { line.textContent = message; line.hidden = false; };

  // -- 1. looking ----------------------------------------------------------------------------------

  async function look() {
    if (!api().canAddModels()) { cannot(); return; }
    show(tr('setup.look.title'), tr('setup.look.lead'), el('div', 'cedar-setup-spinner'));
    const { presets = [] } = await api().servers();
    const within = (promise) => Promise.race([promise, new Promise((_, no) => setTimeout(() => no(new Error('timeout')), LOOK_MS))]);
    const answers = await Promise.all(presets.map((p) => within(api().modelsAt(p.base_url))
      .then((models) => (models.length ? { name: p.name, baseUrl: p.base_url, models } : null))
      .catch(() => null)));
    found = answers.filter(Boolean);
    if (!root) return; // closed meanwhile
    if (found.length) pick(found[0]);
    else choose();
  }

  // -- 2. a model was found, or one is chosen ------------------------------------------------------

  /** The models of one server, the suggested one picked; `apiKey` when it is a hosted provider's. */
  function pick(server, { apiKey = '', wanted = [], back = null } = {}) {
    const select = el('select');
    const suggested = api().suggestedAmong(server.models, wanted);
    for (const name of server.models) select.append(new Option(name, name));
    select.value = suggested;
    const error = errorLine();
    const use = button(tr('setup.pick.use'), async () => {
      use.disabled = true;
      use.textContent = tr('setup.pick.checking');
      error.hidden = true;
      try {
        await api().addFirstModel({ baseUrl: server.baseUrl, apiKey, model: select.value });
        ready(select.value.split('/').pop());
      } catch (e) {
        fail(error, e.message);
        use.disabled = false;
        use.textContent = tr('setup.pick.use');
      }
    }, 'cedar-setup-btn primary');
    const others = found.filter((s) => s !== server && !apiKey);
    const row = el('div', 'cedar-setup-actions');
    row.append(use);
    for (const other of others) row.append(button(tr('setup.pick.other', { server: other.name }), () => pick(other)));
    row.append(button(tr(back ? 'setup.back' : 'setup.pick.else'), back || choose, 'cedar-setup-link'));
    show(
      tr('setup.pick.title', { server: server.name }),
      tr(server.models.length > 1 ? 'setup.pick.lead' : 'setup.pick.leadOne'),
      server.models.length > 1 ? field(tr('setup.pick.model'), select) : el('p', 'cedar-setup-model', server.models[0]),
      error, row,
    );
  }

  /** Nothing runs here, or something else is wanted: three ways to get a model. */
  function choose() {
    const cards = el('div', 'cedar-setup-cards');
    const card = (title, text, onClick) => {
      const b = button('', onClick, 'cedar-setup-card');
      b.append(el('strong', '', title), el('span', '', text));
      cards.append(b);
    };
    const app = api().appModel();
    if (app.available) card(tr('setup.choose.download'), tr('setup.choose.downloadText'), app.open);
    card(tr('setup.choose.hosted'), tr('setup.choose.hostedText'), hosted);
    card(tr('setup.choose.own'), tr('setup.choose.ownText'), own);
    const row = el('div', 'cedar-setup-actions');
    row.append(button(tr('setup.choose.again'), look));
    if (found.length) row.append(button(tr('setup.back'), () => pick(found[0]), 'cedar-setup-link'));
    show(
      tr(found.length ? 'setup.choose.titleElse' : 'setup.choose.title'),
      tr(found.length ? 'setup.choose.leadElse' : app.available ? 'setup.choose.leadApp' : 'setup.choose.lead'),
      cards, row,
    );
  }

  /** A hosted provider: which one, and its key. Cedar lists its models and suggests one. */
  async function hosted() {
    const { providers = [] } = await api().servers();
    const provider = el('select');
    for (const p of providers) provider.append(new Option(p.name, p.base_url));
    const key = input('password', tr('setup.hosted.keyPlaceholder'));
    const docs = el('a', 'cedar-setup-docs');
    docs.target = '_blank';
    docs.rel = 'noopener noreferrer';
    const chosen = () => providers.find((p) => p.base_url === provider.value) || providers[0];
    const sync = () => { docs.href = chosen().docs; docs.textContent = tr('setup.hosted.docs', { provider: chosen().name }); };
    provider.addEventListener('change', sync);
    sync();
    const error = errorLine();
    const form = el('form', 'cedar-setup-form');
    const go = button(tr('setup.continue'), () => form.requestSubmit(), 'cedar-setup-btn primary');
    const row = el('div', 'cedar-setup-actions');
    row.append(go, button(tr('setup.back'), choose, 'cedar-setup-link'));
    form.append(field(tr('setup.hosted.provider'), provider), field(tr('setup.hosted.key'), key), docs, error, row);
    form.addEventListener('submit', (e) => {
      e.preventDefault();
      if (!key.value.trim()) { key.focus(); return; }
      const p = chosen();
      connect({ name: p.name, baseUrl: p.base_url, apiKey: key.value.trim(), wanted: p.suggested, go, error, back: hosted });
    });
    show(tr('setup.hosted.title'), tr('setup.hosted.lead'), form);
    key.focus();
  }

  /** A model server somewhere else: its address, and a key if it wants one. */
  function own() {
    const address = input('url', 'http://192.168.1.20:11434');
    const key = input('password', tr('setup.own.keyPlaceholder'));
    const error = errorLine();
    const form = el('form', 'cedar-setup-form');
    const go = button(tr('setup.continue'), () => form.requestSubmit(), 'cedar-setup-btn primary');
    const row = el('div', 'cedar-setup-actions');
    row.append(go, button(tr('setup.back'), choose, 'cedar-setup-link'));
    form.append(field(tr('setup.own.address'), address), field(tr('setup.own.key'), key), error, row);
    form.addEventListener('submit', (e) => {
      e.preventDefault();
      if (!address.value.trim()) { address.focus(); return; }
      connect({ name: address.value.trim().replace(/^https?:\/\//, '').replace(/\/.*$/, ''), baseUrl: address.value.trim(), apiKey: key.value.trim(), go, error, back: own });
    });
    show(tr('setup.own.title'), tr('setup.own.lead'), form);
    address.focus();
  }

  /** Ask the server for its models and go on to picking one; a failure stays on the form. */
  async function connect({ name, baseUrl, apiKey, wanted, go, error, back }) {
    go.disabled = true;
    go.textContent = tr('setup.connecting');
    error.hidden = true;
    try {
      const models = await api().modelsAt(baseUrl, apiKey);
      if (!models.length) throw new Error(tr('setup.noModels', { server: name }));
      pick({ name, baseUrl, models }, { apiKey, wanted, back });
    } catch (e) {
      fail(error, e.message);
      go.disabled = false;
      go.textContent = tr('setup.continue');
    }
  }

  // -- 3. ready: a first task ----------------------------------------------------------------------

  function ready(model) {
    const cards = el('div', 'cedar-setup-cards');
    for (const task of TASKS) {
      const b = button('', () => { close(); deps.run(tr(`setup.task.${task}.prompt`)); }, 'cedar-setup-card');
      b.append(el('strong', '', tr(`setup.task.${task}.title`)), el('span', '', tr(`setup.task.${task}.text`)));
      cards.append(b);
    }
    const row = el('div', 'cedar-setup-actions');
    row.append(button(tr('setup.ready.skip'), close, 'cedar-setup-link'));
    show(tr('setup.ready.title'), tr('setup.ready.lead', { model }), cards, row);
  }

  // -- someone else sets it up ---------------------------------------------------------------------

  function cannot() {
    const row = el('div', 'cedar-setup-actions');
    row.append(button(tr('setup.cannot.again'), () => location.reload(), 'cedar-setup-btn primary'));
    if (deps.config()?.admin_enabled) {
      row.append(button(tr('setup.cannot.admin'), () => {
        close();
        const open = document.getElementById('settings-btn');
        open.dataset.page = 'server';
        open.click();
      }));
    }
    show(tr('setup.cannot.title'), tr('setup.cannot.lead'), row);
  }

  // -- the page ------------------------------------------------------------------------------------

  function open() {
    if (root) return;
    root = el('div', 'cedar-setup');
    root.setAttribute('role', 'dialog');
    root.setAttribute('aria-modal', 'true');
    root.setAttribute('aria-label', tr('setup.label'));
    const head = el('header', 'cedar-setup-head');
    const brand = el('span', 'cedar-setup-brand');
    const mark = el('img');
    mark.alt = '';
    mark.src = matchMedia('(prefers-color-scheme: dark)').matches ? '/assets/cedar-mark-on-dark.svg' : '/assets/cedar-mark.svg';
    brand.append(mark, el('span', '', 'Cedar'));
    head.append(brand, button(tr('setup.later'), close, 'cedar-setup-link'));
    body = el('main', 'cedar-setup-body');
    root.append(head, body);
    document.body.append(root);
    document.documentElement.setAttribute('data-setup', '');
    look();
  }

  function close() {
    root?.remove();
    root = null;
    body = null;
    document.documentElement.removeAttribute('data-setup');
  }

  function init(helpers) {
    deps = helpers;
  }

  return { init, open, close };
})();

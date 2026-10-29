'use strict';

// Settings that change what the agent can do, shared by every UI. Three pieces, the first two
// pages of the Settings dialog that settings-shell.js frames:
//
//   - The "Tools" and "Model & prompt" pages: which of cedar's tool groups new conversations
//     get, the default model, turn limit and system prompt (GET/PUT /api/settings). Both edit one
//     draft and save it through the dialog's footer.
//   - The "Server" page, for admins only (CHAT_ADMIN_TOKEN): the models cedar has registered
//     (each with its server, API key and reasoning levels), its Brave Search and Best Buy keys and its Google
//     account. The admin token is kept for this browser session and sent as X-Admin-Token. A
//     change to the models reloads cedarModels (models.js) and dispatches `cedar-models-changed`
//     on `document`, so the apps refill their pickers.
//   - A "Tools" button next to the model pickers that changes the tools of one conversation
//     (PUT /api/conversations/{id}/tools). In a new chat the choice is applied when it is created.
//
// Each app hands over its helpers through cedarSettings.init():
//
//   api(method, path, body) -> Promise<json>   the app's /api client (sends the access token)
//   authHeaders() -> object                    the access token header, for the admin calls
//   toast(message)                             a short notice
//   config() -> object                         /api/config (admin_enabled)
//   current() -> conversation | null           the open conversation
//   anchor                                     the element the Tools button goes after (the reasoning picker)

const cedarSettings = (() => {
  const ADMIN_KEY = 'cedar-chat-admin';
  // A model's reply limit when it is registered without one, and the highest cedar takes.
  const DEFAULT_MAX_TOKENS = 16384;
  const MAX_MAX_TOKENS = 1000000;
  const ALL = '*';

  let deps = null;
  let catalog = null;       // /api/tools, once loaded
  let prefs = null;         // /api/settings
  let pending = undefined;  // tools chosen in a new chat before it exists (undefined: none)
  let chip = null;
  let menu = null;
  let integrationsSection = null;

  // -- helpers -----------------------------------------------------------------------------------

  function el(tag, className, text) {
    const e = document.createElement(tag);
    if (className) e.className = className;
    if (text !== undefined) e.textContent = text;
    return e;
  }

  function button(label, onClick, className = 'copy-btn') {
    const b = el('button', className, label);
    b.type = 'button';
    b.addEventListener('click', onClick);
    return b;
  }

  function note(text, className = 'cedar-set-note') {
    return el('p', className, text);
  }

  function adminToken() {
    try { return sessionStorage.getItem(ADMIN_KEY) || ''; } catch { return ''; }
  }

  let memoryAdmin = '';  // used when sessionStorage is unavailable

  function setAdminToken(token) {
    try {
      if (token) sessionStorage.setItem(ADMIN_KEY, token);
      else sessionStorage.removeItem(ADMIN_KEY);
    } catch { /* then it lasts until the page is reloaded */ }
    memoryAdmin = token;
  }

  async function adminApi(method, path, body) {
    const headers = { ...deps.authHeaders(), 'X-Admin-Token': adminToken() || memoryAdmin };
    if (body !== undefined) headers['Content-Type'] = 'application/json';
    const res = await fetch(`/api${path}`, { method, headers, body: body === undefined ? undefined : JSON.stringify(body) });
    if (res.status === 204) return null;
    const data = await res.json().catch(() => null);
    if (!res.ok) {
      const err = new Error(data?.error?.message || tr('settings.requestFailed', { status: res.status }));
      err.status = res.status;
      throw err;
    }
    return data;
  }

  async function loadCatalog(force) {
    if (!catalog || force) catalog = await deps.api('GET', '/tools');
    return catalog;
  }

  // -- tool selection ----------------------------------------------------------------------------
  //
  // A selection is what cedar takes: group ids ("weather"), single tool names ("gmail_get_thread"),
  // or null for every tool. In the UI each group is fully on, partly on (some of its tools), or off.

  function groups() {
    return catalog?.groups || [];
  }

  /** Per group: 'all', 'none', or a Set of the tool names that are on. */
  function decode(tools) {
    const state = new Map();
    for (const g of groups()) {
      if (!tools || tools.includes(ALL) || tools.includes(g.id)) { state.set(g.id, 'all'); continue; }
      const some = new Set(tools.filter((t) => t.startsWith(`${g.id}_`) || (g.tools || []).some((x) => x.name === t)));
      state.set(g.id, some.size ? some : 'none');
    }
    return state;
  }

  /** Back to a selection; null when every group is fully on, so later groups are on too. */
  function encode(state) {
    if ([...state.values()].every((v) => v === 'all')) return null;
    const out = [];
    for (const [id, v] of state) {
      if (v === 'all') out.push(id);
      else if (v instanceof Set) out.push(...v);
    }
    return out;
  }

  function sameSelection(a, b) {
    const norm = (t) => (t === null || t === undefined || t.includes?.(ALL) ? ALL : [...t].sort().join(','));
    return norm(a) === norm(b);
  }

  function writesIn(group) {
    return (group.tools || []).filter((t) => t.writes).map((t) => t.name);
  }

  /** Ask before switching on tools that send or change things. */
  async function confirmWrites(names) {
    if (!names.length) return true;
    return cedarConfirm(tr('settings.confirmWrites', { tools: names.join('\n') }), { ok: tr('settings.allow') });
  }

  /** Checkboxes for every group (and the single tools of Gmail and Calendar); calls onChange(state). */
  function toolPicker(state, onChange, { detailed }) {
    const list = el('div', 'cedar-set-tools');
    for (const g of groups()) {
      const row = el('div', `cedar-set-tool${g.available ? '' : ' unavailable'}`);
      const label = el('label', 'cedar-set-check');
      const box = el('input');
      box.type = 'checkbox';
      const v = state.get(g.id);
      box.checked = v !== 'none';
      box.indeterminate = v instanceof Set;
      const text = el('span');
      text.append(el('strong', '', g.label));
      const sub = g.available
        ? (g.tools?.length ? g.tools.map((t) => t.name).join(', ') : tr('settings.tools.pending'))
        : g.reason || tr('settings.tools.notOffered');
      text.append(el('small', '', sub));
      label.append(box, text);
      row.append(label);
      box.addEventListener('change', async () => {
        if (box.checked && !(await confirmWrites(writesIn(g)))) { box.checked = false; return; }
        state.set(g.id, box.checked ? 'all' : 'none');
        onChange(state);
      });

      const single = detailed && (g.id === 'gmail' || g.id === 'calendar') && g.tools?.length;
      if (single) {
        const details = el('details', 'cedar-set-single');
        details.open = v instanceof Set;
        details.append(el('summary', '', tr('settings.tools.chooseSingle')));
        const inner = el('div', 'cedar-set-single-list');
        for (const t of g.tools) {
          const l = el('label', 'cedar-set-check small');
          const b = el('input');
          b.type = 'checkbox';
          b.checked = v === 'all' || (v instanceof Set && v.has(t.name));
          l.append(b, el('span', '', t.name));
          if (t.writes) l.append(el('span', 'cedar-set-warn', tr('settings.tools.writes')));
          b.addEventListener('change', async () => {
            if (b.checked && t.writes && !(await confirmWrites([t.name]))) { b.checked = false; return; }
            const cur = state.get(g.id);
            const set = cur === 'all' ? new Set(g.tools.map((x) => x.name)) : cur instanceof Set ? new Set(cur) : new Set();
            if (b.checked) set.add(t.name); else set.delete(t.name);
            state.set(g.id, set.size === 0 ? 'none' : set.size === g.tools.length ? 'all' : set);
            onChange(state);
          });
          inner.append(l);
        }
        details.append(inner);
        row.append(details);
      } else if (g.tools?.some((t) => t.writes)) {
        row.append(el('span', 'cedar-set-warn', tr('settings.tools.includesWrites')));
      }
      list.append(row);
    }
    return list;
  }

  // -- Settings: the Tools page and the Model & prompt page --------------------------------------
  //
  // Both pages edit one draft of /api/settings and save it with the same PUT, so saving from
  // either page saves both. The draft is reset every time the dialog opens (each page's render
  // runs then); `loading` shares the one fetch between the two renders.

  const shell = () => (typeof cedarSettingsShell === 'undefined' ? null : cedarSettingsShell);

  let draft = null;        // { tools, model, system, max_turns, plan }, what the pages have edited
  let toolState = null;    // decode(draft.tools), the Tools page's working copy
  let editingPrompt = false;
  let loading = null;      // the fetch shared by the two pages while the dialog opens
  let toolsSection = null;
  let modelSection = null;

  async function loadPrefs() {
    if (!loading) {
      loading = Promise.all([loadCatalog(true), deps.api('GET', '/settings')]).then(([cat, p]) => {
        catalog = cat;
        prefs = p;
        draft = { tools: p.tools, model: p.model, system: p.system, max_turns: p.max_turns, plan: p.plan || 'auto' };
        toolState = decode(p.tools);
        editingPrompt = !isDefaultPrompt();
      });
      loading.finally(() => { setTimeout(() => { loading = null; }, 0); });
    }
    return loading;
  }

  const defaultSystem = () => prefs?.defaults?.system ?? '';
  const isDefaultPrompt = () => (draft?.system ?? defaultSystem()).trim() === defaultSystem().trim();

  /** How many groups the Tools page has changed from the saved defaults. */
  function toolChanges() {
    const saved = decode(prefs.tools);
    let n = 0;
    for (const [id, v] of toolState) {
      const s = saved.get(id);
      const same = v === s || (v instanceof Set && s instanceof Set && v.size === s.size && [...v].every((t) => s.has(t)));
      if (!same) n++;
    }
    return n;
  }

  async function renderTools(section) {
    toolsSection = section;
    section.replaceChildren(note(tr('settings.loading')));
    try { await loadPrefs(); } catch (e) {
      section.replaceChildren(note(tr('settings.loadFailed', { error: e.message }), 'cedar-set-error'));
      return;
    }
    const list = el('div', 'cedar-set-tools');
    const always = el('div', 'cedar-set-always');
    always.append(el('span', 'settings-eyebrow', tr('settings.tools.alwaysOn')), el('span', '', tr('settings.tools.alwaysOnList')));
    list.append(always);
    const changed = () => {
      draft.tools = encode(toolState);
      shell()?.setDirty('tools', toolChanges());
    };
    for (const g of groups()) list.append(toolRow(g, changed));
    section.replaceChildren(list);
    const registry = await registryBlock();
    if (registry && list.isConnected) section.append(registry);
  }

  /** Under the tool list: the community registry's tools, with a link to it, to install from.
   *  `null` without the admin token (its address and its tools come from cedar's admin API) or
   *  when cedar has no registry. */
  async function registryBlock() {
    if (!deps.config().admin_enabled || !unlocked()) return null;
    const market = await marketplaceBlock();
    if (!market) return null;
    const b = el('div', 'cedar-set-block');
    const head = el('div', 'cedar-set-block-head');
    head.append(el('span', 'settings-eyebrow', tr('settings.registry.title')));
    const url = registryPage(lastRegistry);
    if (url) {
      const link = el('a', 'settings-link', tr('settings.registry.open'));
      link.href = url;
      link.target = '_blank';
      link.rel = 'noreferrer';
      head.append(link);
    }
    b.append(head, market);
    return b;
  }

  /** The registry's page on GitHub for an index at raw.githubusercontent.com, else the index. */
  function registryPage(index) {
    const m = /^https:\/\/raw\.githubusercontent\.com\/([^/]+)\/([^/]+)\/([^/]+)\/(?:(.+)\/)?index\.json$/.exec(index || '');
    if (!m) return index || '';
    return `https://github.com/${m[1]}/${m[2]}/tree/${m[3]}${m[4] ? `/${m[4]}` : ''}`;
  }

  /** One switch row of the Tools page. */
  function toolRow(g, onChange) {
    const row = el('div', `cedar-set-row${g.available ? '' : ' unavailable'}`);
    const sw = el('button', 'cedar-set-switch');
    sw.type = 'button';
    sw.setAttribute('role', 'switch');
    sw.setAttribute('aria-label', g.label);
    const main = el('div', 'cedar-set-row-main');
    const sub = el('span', 'cedar-set-row-sub');
    const side = el('span', 'cedar-set-row-side');
    const single = (g.id === 'gmail' || g.id === 'calendar') && g.tools?.length;
    let singles = null;
    const syncSwitch = () => {
      const v = toolState.get(g.id);
      sw.setAttribute('aria-checked', v === 'all' ? 'true' : v === 'none' ? 'false' : 'mixed');
      if (singles) {
        for (const box of singles.querySelectorAll('input')) box.checked = v === 'all' || (v instanceof Set && v.has(box.value));
      }
    };
    sw.addEventListener('click', async () => {
      const on = toolState.get(g.id) === 'none';
      if (on && !(await confirmWrites(writesIn(g)))) return;
      toolState.set(g.id, on ? 'all' : 'none');
      syncSwitch();
      onChange();
    });
    main.append(el('span', 'cedar-set-row-name', g.label));
    if (!g.available) {
      sub.textContent = '';
      side.textContent = g.reason || tr('settings.tools.notOffered');
    } else if (single) {
      sub.append(`${tr('settings.tools.count', { count: g.tools.length })} · `);
      const choose = el('button', 'settings-link', tr('settings.tools.chooseSingleLower'));
      choose.type = 'button';
      choose.setAttribute('aria-expanded', 'false');
      sub.append(choose);
      singles = el('div', 'cedar-set-single-list');
      singles.hidden = !(toolState.get(g.id) instanceof Set);
      choose.setAttribute('aria-expanded', String(!singles.hidden));
      choose.addEventListener('click', () => { singles.hidden = !singles.hidden; choose.setAttribute('aria-expanded', String(!singles.hidden)); });
      for (const t of g.tools) {
        const l = el('label', 'cedar-set-check small');
        const b = el('input');
        b.type = 'checkbox';
        b.value = t.name;
        l.append(b, el('span', '', t.name));
        if (t.writes) l.append(el('span', 'cedar-set-warn', tr('settings.tools.writes')));
        b.addEventListener('change', async () => {
          if (b.checked && t.writes && !(await confirmWrites([t.name]))) { b.checked = false; return; }
          const cur = toolState.get(g.id);
          const set = cur === 'all' ? new Set(g.tools.map((x) => x.name)) : cur instanceof Set ? new Set(cur) : new Set();
          if (b.checked) set.add(t.name); else set.delete(t.name);
          toolState.set(g.id, set.size === 0 ? 'none' : set.size === g.tools.length ? 'all' : set);
          syncSwitch();
          onChange();
        });
        singles.append(l);
      }
    } else {
      sub.textContent = g.tools?.length ? g.tools.map((t) => t.name).join(', ') : tr('settings.tools.pending');
    }
    if (g.available && g.tools?.some((t) => t.writes)) {
      side.textContent = tr('settings.tools.canWrite');
      side.classList.add('warn');
    }
    main.append(sub);
    row.append(sw, main, side);
    syncSwitch();
    if (!singles) return row;
    const wrap = el('div');
    wrap.append(row, singles);
    return wrap;
  }

  async function renderModel(section) {
    modelSection = section;
    section.replaceChildren(note(tr('settings.loading')));
    try { await loadPrefs(); } catch (e) {
      section.replaceChildren(note(tr('settings.loadFailed', { error: e.message }), 'cedar-set-error'));
      return;
    }
    const defaults = prefs.defaults || null;
    const dirty = () => shell()?.setDirty('model', true);
    const parts = [];

    const fields = el('div', 'cedar-set-fields');
    if (cedarModels.all().length) {
      const modelSelect = el('select');
      const def = cedarModels.byId(cedarModels.defaultId());
      modelSelect.append(new Option(def ? tr('settings.model.cedarDefaultIs', { model: modelLabel(def) }) : tr('settings.model.cedarDefault'), ''));
      for (const m of cedarModels.all()) modelSelect.append(new Option(modelLabel(m), m.id));
      modelSelect.value = cedarModels.byId(draft.model) ? draft.model : '';
      modelSelect.addEventListener('change', () => { draft.model = modelSelect.value || null; dirty(); });
      fields.append(field(tr('settings.model.defaultModel'), modelSelect));
    } else if (cedarModels.loaded()) {
      parts.push(note(tr('settings.model.noneRegistered'), 'cedar-set-error'));
    }
    const turns = el('input');
    turns.type = 'number';
    turns.min = '1';
    turns.max = '1000';
    turns.placeholder = defaults ? tr('settings.model.serverLimitIs', { limit: String(defaults.max_turns_cap) }) : tr('settings.model.serverLimit');
    turns.value = draft.max_turns ?? '';
    turns.addEventListener('input', () => { draft.max_turns = turns.value ? Number(turns.value) : null; dirty(); });
    fields.append(field(tr('settings.model.maxTurns'), turns));
    const plan = el('select');
    for (const value of ['auto', 'tool', 'on', 'off']) plan.append(new Option(tr(`settings.plan.${value}`), value));
    plan.value = draft.plan || 'auto';
    plan.addEventListener('change', () => { draft.plan = plan.value; dirty(); });
    fields.append(field(tr('settings.plan.title'), plan));
    parts.push(fields);
    const notes = el('div', 'cedar-set-field-notes');
    notes.append(el('div', 'cedar-set-field-note', tr('settings.plan.note')));
    parts.push(notes);

    // The system prompt: read as a document, edited as one. Without a saved prompt it shows
    // cedar's default text, which is saved as "unset" so it keeps following cedar's SYSTEM_PROMPT.
    const block = el('div', 'cedar-set-prompt-block');
    const head = el('div', 'cedar-set-prompt-head');
    const headLeft = el('span');
    const tag = el('span', 'cedar-set-badge', tr('settings.model.cedarDefault'));
    headLeft.append(el('span', 'settings-eyebrow', tr('settings.prompt.title')), tag);
    const acts = el('div', 'cedar-set-prompt-acts');
    head.append(headLeft, acts);
    const reading = el('div', 'cedar-set-prompt');
    const editor = el('textarea', 'cedar-set-prompt-edit');
    editor.setAttribute('aria-label', tr('settings.prompt.title'));
    editor.placeholder = defaults ? '' : tr('settings.prompt.unknownDefault');
    const text = () => draft.system ?? defaultSystem();
    const sync = () => {
      tag.hidden = !isDefaultPrompt();
      reading.textContent = text();
      editor.value = text();
      reading.hidden = editingPrompt;
      editor.hidden = !editingPrompt;
      acts.textContent = '';
      if (editingPrompt) {
        if (!isDefaultPrompt()) acts.append(button(tr('settings.prompt.useDefault'), () => { draft.system = null; editingPrompt = false; sync(); dirty(); }, 'settings-ghost'));
        acts.append(button(tr('settings.prompt.done'), () => { editingPrompt = false; sync(); }, 'settings-ghost'));
      } else {
        acts.append(button(tr('settings.edit'), () => { editingPrompt = true; sync(); editor.focus(); }, 'settings-ghost'));
      }
    };
    editor.addEventListener('input', () => {
      draft.system = editor.value;
      tag.hidden = !isDefaultPrompt();
      dirty();
    });
    sync();
    block.append(head, reading, editor, el('div', 'cedar-set-field-note', tr('settings.prompt.note')));
    parts.push(block);
    section.replaceChildren(...parts);
  }

  const modelLabel = (m) => {
    const name = m.label || m.model || m.id;
    return m.default_reasoning ? `${name} · ${m.default_reasoning}` : name;
  };

  /** PUT the draft; both pages save through this. Resolves when saved, rejects after a toast. */
  async function submit() {
    if (!draft) return;
    const system = (draft.system ?? '').trim();
    const body = {
      tools: draft.tools,
      model: draft.model || null,
      system: isDefaultPrompt() || !system ? null : system,
      max_turns: draft.max_turns ?? null,
      plan: draft.plan || 'auto',
    };
    try {
      const defaults = prefs.defaults;
      prefs = { ...(await deps.api('PUT', '/settings', body)), defaults };
      draft = { tools: prefs.tools, model: prefs.model, system: prefs.system, max_turns: prefs.max_turns, plan: prefs.plan || 'auto' };
      toolState = decode(prefs.tools);
      editingPrompt = !isDefaultPrompt();
      updateChip();
      const s = shell();
      s?.setDirty('tools', 0);
      s?.setDirty('model', 0);
      if (toolsSection) renderTools(toolsSection);
      if (modelSection) renderModel(modelSection);
      s?.refresh('tools');
      s?.refresh('model');
    } catch (e) {
      deps.toast(e.message);
      throw e;
    }
  }

  function toolsSummary() {
    if (!prefs || !catalog) return null;
    if (prefs.tools === null || prefs.tools === undefined) return tr('settings.tools.summaryAll');
    const state = decode(prefs.tools);
    const on = [...state.values()].filter((v) => v !== 'none').length;
    return tr('settings.tools.summary', { on, count: state.size });
  }

  function modelSummary() {
    if (!prefs) return null;
    if (cedarModels.loaded() && !cedarModels.all().length) return { text: tr('settings.model.summaryNone'), kind: 'error' };
    const m = cedarModels.byId(prefs.model) || cedarModels.byId(cedarModels.defaultId());
    const parts = [];
    if (m) parts.push(m.label || m.model || m.id);
    if (m?.default_reasoning) parts.push(m.default_reasoning);
    parts.push(prefs.system ? tr('settings.model.customPrompt') : tr('settings.model.defaultPrompt'));
    return parts.join(' · ');
  }

  function field(label, input, extra) {
    const f = el('label', 'cedar-set-field');
    const head = el('span', 'cedar-set-field-head');
    head.append(el('span', '', label));
    if (extra) head.append(extra);
    f.append(head, input);
    return f;
  }

  // -- Settings: integrations (admin) ------------------------------------------------------------

  const unlocked = () => Boolean(adminToken() || memoryAdmin);

  async function renderIntegrations(section = integrationsSection) {
    if (!section) return;
    integrationsSection = section;
    const cfg = deps.config();
    shell()?.refresh('server');
    if (!cfg.admin_enabled) {
      try { await loadCatalog(); } catch { /* the help is shown without the list */ }
      const card = el('div', 'cedar-set-unlock-card');
      card.append(setupHelp());
      section.replaceChildren(card);
      return;
    }
    if (!unlocked()) {
      section.replaceChildren(unlockForm());
      return;
    }
    section.replaceChildren(note(tr('settings.loading')));
    let status;
    try {
      status = await adminApi('GET', '/integrations');
    } catch (e) {
      if (e.status === 403) {
        setAdminToken('');
        shell()?.refresh('server');
        section.replaceChildren(unlockForm(e.message));
      } else {
        section.replaceChildren(note(e.message, 'cedar-set-error'));
      }
      return;
    }
    const parts = [];
    if (!status.storage) {
      parts.push(note(tr('settings.server.noStorage')));
    }
    const block = (title, ...children) => {
      const b = el('div', 'cedar-set-block');
      const head = el('div', 'cedar-set-block-head');
      head.append(el('span', 'settings-eyebrow', title));
      b.append(head, ...children);
      return b;
    };
    const models = await modelsBlock(status);
    const modelsBlockEl = block(tr('settings.server.models'), models.wrap);
    modelsBlockEl.querySelector('.cedar-set-block-head').append(models.add);
    parts.push(modelsBlockEl, block('Brave Search', braveRow(status)), block('Best Buy', bestbuyRow(status)), block(tr('settings.google.title'), googleRow(status)), block(tr('settings.server.attachments'), await imageSizeRow()), block(tr('settings.server.sandbox'), await sandboxIdleRow()));
    const tools = await personalToolsBlock();
    if (tools) parts.push(block(tr('settings.server.personalTools'), tools));
    const market = await marketplaceBlock();
    if (market) parts.push(block(tr('settings.server.communityTools'), market));
    section.replaceChildren(...parts);
  }

  // -- Settings: advanced (admin) ----------------------------------------------------------------

  let advancedSection = null;
  /** cedar's server settings as last loaded for the Advanced page, for its nav line. */
  let advancedSettings = null;

  /** Server-wide options most setups leave alone: for now, what runs the sandboxes. */
  async function renderAdvanced(section = advancedSection) {
    if (!section) return;
    advancedSection = section;
    const cfg = deps.config();
    if (!cfg.admin_enabled || !unlocked()) {
      advancedSettings = null;
      const card = el('div', 'cedar-set-help');
      card.append(note(tr(cfg.admin_enabled ? 'settings.advanced.locked' : 'settings.advanced.noAdmin')));
      if (cfg.admin_enabled) card.append(button(tr('settings.advanced.unlock'), () => shell()?.go('server'), 'primary'));
      section.replaceChildren(card);
      shell()?.refresh('advanced');
      return;
    }
    section.replaceChildren(note(tr('settings.loading')));
    try {
      advancedSettings = await adminApi('GET', '/integrations/settings');
    } catch (e) {
      advancedSettings = null;
      if (e.status === 403) {
        // A wrong token: the Server page asks for it again.
        setAdminToken('');
        return renderAdvanced(section);
      }
      section.replaceChildren(note(e.message, 'cedar-set-error'));
      shell()?.refresh('advanced');
      return;
    }
    const b = el('div', 'cedar-set-block');
    const head = el('div', 'cedar-set-block-head');
    head.append(el('span', 'settings-eyebrow', tr('settings.server.sandbox')));
    b.append(head, openShellRow(advancedSettings));
    section.replaceChildren(b);
    shell()?.refresh('advanced');
  }

  /** The switch between Docker (off, the default) and OpenShell (on) for conversations' sandboxes. */
  function openShellRow(settings) {
    const supported = settings.openshell_supported !== false;
    const on = settings.sandbox_runtime === 'openshell';
    const { row, main, side } = integrationRow(tr('settings.advanced.openshell.title'), tr('settings.advanced.openshell.desc'));
    const sw = el('button', 'cedar-set-switch');
    sw.type = 'button';
    sw.setAttribute('role', 'switch');
    sw.setAttribute('aria-checked', String(on));
    sw.setAttribute('aria-label', tr('settings.advanced.openshell.title'));
    sw.disabled = !supported;
    const state = badge(on ? 'OpenShell' : 'Docker', 'ok');
    const problem = el('small', 'cedar-set-error');
    problem.hidden = true;
    if (!supported) main.append(el('small', 'cedar-set-hint', tr('settings.advanced.openshell.unsupported')));
    main.append(problem);
    const show = (now) => {
      sw.setAttribute('aria-checked', String(now));
      state.textContent = now ? 'OpenShell' : 'Docker';
    };
    sw.addEventListener('click', async () => {
      const was = sw.getAttribute('aria-checked') === 'true';
      sw.disabled = true;
      problem.hidden = true;
      state.textContent = tr('settings.advanced.openshell.checking');
      try {
        advancedSettings = await adminApi('PUT', '/integrations/settings', { sandbox_runtime: was ? null : 'openshell' });
        const now = advancedSettings.sandbox_runtime === 'openshell';
        show(now);
        deps.toast(tr(now ? 'settings.advanced.openshell.savedOn' : 'settings.advanced.openshell.savedOff'));
      } catch (err) {
        show(was);
        problem.textContent = err.message;
        problem.hidden = false;
      } finally {
        sw.disabled = !supported;
        shell()?.refresh('advanced');
      }
    });
    side.append(state, sw);
    return row;
  }

  /** The Advanced page's nav line: the sandbox runtime, once known. */
  function advancedSummary() {
    if (!deps?.config()?.admin_enabled || !unlocked()) return tr('shell.tag.ADMIN');
    if (!advancedSettings) return null;
    return advancedSettings.sandbox_runtime === 'openshell' ? 'OpenShell' : 'Docker';
  }

  /** The page head's Lock button, when the admin token is in. */
  function serverActions() {
    if (!deps?.config()?.admin_enabled || !unlocked()) return null;
    return [button(tr('settings.server.lock'), () => { setAdminToken(''); renderIntegrations(); renderAdvanced(); shell()?.refresh('server'); }, 'settings-ghost')];
  }

  /** Where keys go when this page can't manage them (no CHAT_ADMIN_TOKEN). */
  function setupHelp() {
    const wrap = el('div', 'cedar-set-help');
    wrap.append(note(tr('settings.setup.intro')));
    const list = el('ul');
    const item = (code, text) => {
      const li = el('li');
      li.append(el('code', '', code), ` ${text}`);
      list.append(li);
    };
    item('CHAT_ADMIN_TOKEN', tr('settings.setup.adminToken'));
    item('CEDAR_SECRET_KEY', tr('settings.setup.secretKey'));
    item('CEDAR_ADMIN_KEYS', tr('settings.setup.adminKeys'));
    wrap.append(list);
    wrap.append(note(tr('settings.setup.where')));
    if (cedarModels.loaded() && !cedarModels.all().length) {
      wrap.append(note(tr('settings.setup.noModel'), 'cedar-set-error'));
    }
    const missing = groups().filter((g) => !g.available);
    if (missing.length) {
      const l = el('ul');
      for (const g of missing) {
        const li = el('li');
        li.append(el('strong', '', g.label), `: ${g.reason || tr('settings.setup.notOffered')}`);
        l.append(li);
      }
      wrap.append(note(tr('settings.setup.unavailable')), l);
    }
    return wrap;
  }

  function unlockForm(error) {
    const form = el('form', 'cedar-set-unlock');
    const input = el('input');
    input.type = 'password';
    input.autocomplete = 'off';
    input.placeholder = tr('settings.unlock.token');
    input.setAttribute('aria-label', tr('settings.unlock.token'));
    form.append(input, button(tr('settings.unlock.button'), () => form.requestSubmit(), 'primary'));
    form.addEventListener('submit', (e) => {
      e.preventDefault();
      if (!input.value.trim()) return;
      setAdminToken(input.value.trim());
      renderIntegrations();
      renderAdvanced();
      refreshTools();
    });
    const wrap = el('div', 'cedar-set-unlock-card');
    wrap.append(note(tr('settings.unlock.intro')));
    if (cedarModels.loaded() && !cedarModels.all().length) {
      wrap.append(note(tr('settings.unlock.noModel'), 'cedar-set-error'));
    }
    if (error) wrap.append(note(error, 'cedar-set-error'));
    wrap.append(form);
    return wrap;
  }

  function badge(text, kind = '') {
    return el('span', `cedar-set-badge ${kind}`, text);
  }

  function integrationRow(title, description) {
    const row = el('div', 'cedar-set-integration');
    const main = el('div', 'cedar-set-integration-main');
    main.append(el('strong', '', title), el('small', '', description));
    const side = el('div', 'cedar-set-integration-side');
    row.append(main, side);
    return { row, main, side };
  }

  // -- the model registry --------------------------------------------------------------------------

  /** Tell the apps the registry changed, so their pickers follow. */
  async function modelsChanged() {
    try { await cedarModels.load(deps.api); } catch { /* the next load retries */ }
    document.dispatchEvent(new CustomEvent('cedar-models-changed'));
  }

  /**
   * The models cedar has registered, each with its server, and a form to add one. Every entry can
   * be edited, made the default, or removed; the first one registered is the default.
   */
  async function modelsBlock(status) {
    const wrap = el('div', 'cedar-set-models');
    let entries;
    try {
      ({ models: entries } = await adminApi('GET', '/integrations/models'));
    } catch (e) {
      wrap.append(note(tr('settings.models.listFailed', { error: e.message }), 'cedar-set-error'));
      return { wrap, add: el('span') };
    }
    entries = entries || [];
    // The button sits in the block's heading, so it is in view however long the list is. The form
    // it opens goes right under the intro, above the first entry.
    const intro = entries.length
      ? note(tr('settings.models.intro'))
      : note(tr('settings.models.introEmpty'), 'cedar-set-error');
    wrap.append(intro);
    const add = button(tr('settings.models.add'), () => {
      add.hidden = true;
      const form = modelForm(null, status, () => { add.hidden = false; form.remove(); });
      intro.after(form);
      form.querySelector('input').focus();
    }, entries.length ? 'settings-ghost' : 'primary');
    add.setAttribute('aria-label', tr('settings.models.add'));
    for (const entry of entries) wrap.append(modelRow(entry, status));
    return { wrap, add };
  }

  function modelRow(entry, status) {
    const levels = entry.reasoning_levels || [];
    const { row, main, side } = integrationRow(entry.label || entry.id, tr('settings.models.at', { model: entry.model, url: entry.base_url }));
    main.append(el('small', '', tr('settings.models.id', { id: entry.id })));
    if (entry.is_default) side.append(badge(tr('settings.models.default'), 'ok'));
    if (entry.api_key_set) side.append(badge(tr('settings.models.keySet')));
    if (entry.multimodal) side.append(badge(tr('settings.models.images')));
    side.append(badge(levels.length
      ? (entry.default_reasoning
        ? tr('settings.models.levelsDefault', { levels: levels.join(', '), level: entry.default_reasoning })
        : tr('settings.models.levels', { levels: levels.join(', ') }))
      : tr('settings.models.noLevels')));
    if (entry.max_tokens) side.append(badge(tr('settings.models.maxTokensBadge', { count: entry.max_tokens })));
    // What the server listed when the entry was checked is offered in the edit form; the row only
    // says when the model name is not one of them.
    if (Array.isArray(entry.listed_models) && !entry.listed_models.includes(entry.model)) {
      const listed = entry.listed_models.length ? entry.listed_models.join(', ') : tr('settings.models.listedNone');
      main.append(el('small', 'cedar-set-warn-text', tr('settings.models.notListed', { model: entry.model, listed })));
    }
    let form = null;
    side.append(button(tr('settings.edit'), () => {
      if (form) { form.remove(); form = null; return; }
      form = modelForm(entry, status, () => { form?.remove(); form = null; });
      row.append(form);
      form.querySelector('input').focus();
    }));
    if (!entry.is_default) {
      side.append(button(tr('settings.models.makeDefault'), async () => {
        try {
          await adminApi('PUT', `/integrations/models/${encodeURIComponent(entry.id)}`, { base_url: entry.base_url, model: entry.model, is_default: true });
          deps.toast(tr('settings.models.isDefault', { name: entry.label || entry.id }));
          await modelsChanged();
          renderIntegrations();
        } catch (err) { deps.toast(err.message); }
      }));
    }
    side.append(button(tr('settings.remove'), async () => {
      if (!(await cedarConfirm(tr('settings.models.confirmRemove', { name: entry.label || entry.id }), { ok: tr('settings.remove') }))) return;
      try {
        await adminApi('DELETE', `/integrations/models/${encodeURIComponent(entry.id)}`);
        await modelsChanged();
        renderIntegrations();
      } catch (err) { deps.toast(err.message); }
    }));
    return row;
  }

  /** A form to register a model (`entry` null) or change one. `onDone` closes it. */
  function modelForm(entry, status, onDone) {
    const form = el('form', 'cedar-set-inline cedar-set-endpoint');
    const input = (placeholder, value, label, type = 'text') => {
      const i = el('input');
      i.type = type;
      i.placeholder = placeholder;
      i.value = value;
      i.setAttribute('aria-label', label);
      i.spellcheck = false;
      i.autocapitalize = 'off';
      return i;
    };
    const slug = (text) => text.toLowerCase().replace(/[^a-z0-9._-]+/g, '-').replace(/^[^a-z0-9]+|-+$/g, '').slice(0, 64);
    const id = input(tr('settings.form.idPlaceholder'), entry?.id || '', tr('settings.form.id'));
    id.disabled = Boolean(entry);
    const label = input(tr('settings.form.labelPlaceholder'), entry?.label || '', tr('settings.form.label'));
    let idTouched = Boolean(entry);
    id.addEventListener('input', () => { idTouched = true; });
    label.addEventListener('input', () => { if (!idTouched) id.value = slug(label.value); });
    // The whole address, protocol and path included; cedar adds /v1 if the path is left off.
    const address = input(tr('settings.form.addressPlaceholder'), entry?.base_url || '', tr('settings.form.address'));
    const canSaveKey = Boolean(status.storage);
    const key = input(
      !canSaveKey ? tr('settings.form.keyNoStorage') : entry?.api_key_set ? tr('settings.form.keyKeep') : tr('settings.form.keyOptional'),
      '', tr('settings.form.key'), 'password',
    );
    key.autocomplete = 'off';
    key.disabled = !canSaveKey;
    const clearKey = el('label', 'cedar-set-check small plain');
    const clearBox = el('input');
    clearBox.type = 'checkbox';
    clearKey.append(clearBox, el('span', '', tr('settings.form.clearKey')));
    // The model name is picked from what the server lists: "List models" asks it (with the key
    // typed above, else the entry's saved one), and the picker offers those names only. An entry
    // being edited starts with what its server listed when it was last checked.
    const model = el('select');
    model.setAttribute('aria-label', tr('settings.form.modelName'));
    const listBtn = button(tr('settings.form.list'), () => listServerModels());
    const modelRow = el('div', 'cedar-set-server-models');
    modelRow.append(model, listBtn);
    const fillModels = (names, selected) => {
      model.textContent = '';
      if (!names) {
        model.append(new Option(tr('settings.form.listFirst'), ''));
        model.disabled = true;
        return;
      }
      model.disabled = false;
      const options = [...names];
      if (selected && !options.includes(selected)) options.unshift(selected);
      model.append(new Option(options.length ? tr('settings.form.choose') : tr('settings.form.noModels'), ''));
      for (const m of options) model.append(new Option(m === selected && !names.includes(m) ? tr('settings.form.unlisted', { model: m }) : m, m));
      model.value = selected && options.includes(selected) ? selected : options.length === 1 ? options[0] : '';
    };
    fillModels(entry ? entry.listed_models || [entry.model] : null, entry?.model || '');
    // A new entry without a label or id yet takes its id from the chosen name.
    model.addEventListener('change', () => { if (!idTouched && !label.value.trim()) id.value = slug(model.value.split('/').pop()); });
    // A changed address or key may serve other models: list again before saving.
    const stale = () => { if (!model.disabled) fillModels(null); };
    address.addEventListener('input', stale);
    key.addEventListener('input', stale);
    clearBox.addEventListener('change', stale);
    // Enter in the address or key lists the models rather than saving the form.
    for (const i of [address, key]) {
      i.addEventListener('keydown', (e) => { if (e.key === 'Enter') { e.preventDefault(); listServerModels(); } });
    }
    async function listServerModels() {
      if (!address.value.trim()) { deps.toast(tr('settings.form.needAddress')); address.focus(); return; }
      const body = { base_url: address.value.trim() };
      if (key.value.trim()) body.api_key = key.value.trim();
      else if (entry && !clearBox.checked) body.id = entry.id;
      const previous = model.value || entry?.model || '';
      listBtn.disabled = true;
      listBtn.textContent = tr('settings.form.listing');
      try {
        const found = await adminApi('POST', '/integrations/server-models', body);
        fillModels(found.models || [], found.models?.includes(previous) ? previous : '');
        model.dispatchEvent(new Event('change'));
        if (!found.models?.length) deps.toast(tr('settings.form.noModels'));
        model.focus();
      } catch (err) {
        fillModels(null);
        deps.toast(err.message);
      } finally {
        listBtn.disabled = false;
        listBtn.textContent = tr('settings.form.list');
      }
    }
    const levels = input(tr('settings.form.levelsPlaceholder'), (entry?.reasoning_levels || []).join(', '), tr('settings.form.levels'));
    const defaultLevel = el('select');
    defaultLevel.setAttribute('aria-label', tr('settings.form.defaultLevel'));
    const parseLevels = () => [...new Set(levels.value.split(',').map((l) => l.trim()).filter(Boolean))];
    const fillDefault = () => {
      const current = defaultLevel.value || entry?.default_reasoning || '';
      defaultLevel.textContent = '';
      defaultLevel.append(new Option(tr('settings.form.defaultLevelServer'), ''));
      for (const l of parseLevels()) defaultLevel.append(new Option(tr('settings.form.defaultLevelIs', { level: l }), l));
      defaultLevel.value = parseLevels().includes(current) ? current : '';
      defaultLevel.hidden = !parseLevels().length;
    };
    levels.addEventListener('input', fillDefault);
    fillDefault();
    const maxTokens = input(tr('settings.form.maxTokens'), String(entry?.max_tokens ?? DEFAULT_MAX_TOKENS), tr('settings.form.maxTokens'), 'number');
    maxTokens.min = '1';
    maxTokens.max = String(MAX_MAX_TOKENS);
    maxTokens.step = '1';
    const maxTokensField = el('label', 'cedar-set-field');
    const maxTokensHead = el('span', 'cedar-set-field-head');
    maxTokensHead.append(el('span', '', tr('settings.form.maxTokens')));
    maxTokensField.append(maxTokensHead, maxTokens, el('small', 'cedar-set-field-note', tr('settings.form.maxTokensNote')));
    const multimodalLabel = el('label', 'cedar-set-check plain');
    const multimodalBox = el('input');
    multimodalBox.type = 'checkbox';
    multimodalBox.checked = Boolean(entry?.multimodal);
    const multimodalText = el('span');
    multimodalText.append(el('strong', '', tr('settings.form.images')));
    multimodalText.append(el('small', '', tr('settings.form.imagesNote')));
    multimodalLabel.append(multimodalBox, multimodalText);
    const makeDefault = el('label', 'cedar-set-check small plain');
    const defaultBox = el('input');
    defaultBox.type = 'checkbox';
    defaultBox.checked = Boolean(entry?.is_default);
    defaultBox.disabled = Boolean(entry?.is_default);
    makeDefault.append(defaultBox, el('span', '', tr('settings.form.makeDefault')));
    const saveBtn = button(entry ? tr('settings.checkSave') : tr('settings.form.checkAdd'), () => form.requestSubmit(), 'primary');
    form.append(id, label, address, key);
    if (entry?.api_key_set && canSaveKey) form.append(clearKey);
    form.append(modelRow);
    form.append(levels, defaultLevel, maxTokensField, multimodalLabel, makeDefault);
    form.append(el('small', 'cedar-set-field-note', tr('settings.form.note')));
    form.append(saveBtn, button(tr('settings.cancel'), onDone));
    form.addEventListener('submit', async (e) => {
      e.preventDefault();
      if (!id.value.trim()) { deps.toast(tr('settings.form.needId')); id.focus(); return; }
      if (!address.value.trim()) { deps.toast(tr('settings.form.needAddress')); address.focus(); return; }
      if (model.disabled) { deps.toast(tr('settings.form.needList')); listBtn.focus(); return; }
      if (!model.value) { deps.toast(tr('settings.form.choose')); model.focus(); return; }
      const limit = Number(maxTokens.value);
      if (!Number.isInteger(limit) || limit < 1 || limit > MAX_MAX_TOKENS) {
        deps.toast(tr('settings.form.maxTokensRange', { max: MAX_MAX_TOKENS }));
        maxTokens.focus();
        return;
      }
      const body = {
        base_url: address.value.trim(),
        model: model.value,
        label: label.value.trim() || model.value,
        reasoning_levels: parseLevels(),
        default_reasoning: defaultLevel.value,
        multimodal: multimodalBox.checked,
        max_tokens: limit,
        is_default: defaultBox.checked,
      };
      if (key.value.trim()) body.api_key = key.value.trim();
      else if (clearBox.checked) body.api_key = '';
      saveBtn.disabled = true;
      saveBtn.textContent = tr('settings.checking');
      try {
        const saved = await adminApi('PUT', `/integrations/models/${encodeURIComponent(id.value.trim())}`, body);
        deps.toast(`${saved.label || saved.id}: ${tr('settings.models.at', { model: saved.model, url: saved.base_url })}`);
        await modelsChanged();
        renderIntegrations();
        refreshTools();
      } catch (err) {
        saveBtn.disabled = false;
        saveBtn.textContent = entry ? tr('settings.checkSave') : tr('settings.form.checkAdd');
        deps.toast(err.message);
      }
    });
    return form;
  }

  function braveRow(status) {
    return keyRow(status.brave, status.storage, {
      name: 'Brave Search',
      what: tr('settings.brave.what'),
      path: '/integrations/brave',
      saved: tr('settings.brave.saved'),
      removeAsk: tr('settings.brave.remove'),
    });
  }

  function bestbuyRow(status) {
    const b = status.bestbuy || {};
    if (b.enabled === false) {
      const { row, side } = integrationRow('Best Buy', tr('settings.bestbuy.off'));
      side.append(badge(tr('settings.off')));
      return row;
    }
    return keyRow(b, status.storage, {
      name: 'Best Buy',
      what: tr('settings.bestbuy.what'),
      path: '/integrations/bestbuy',
      saved: tr('settings.bestbuy.saved'),
      removeAsk: tr('settings.bestbuy.remove'),
    });
  }

  /**
   * A row for an API key cedar keeps for a tool: its last four characters and where it is set,
   * and, when cedar can store keys and the environment doesn't set it, a form to add or replace
   * it (cedar checks it first) and a button to remove it. `k` is the key's entry in cedar's
   * integration status, `{configured, source, last4}`.
   */
  function keyRow(k = {}, storage, { name, what, path, saved, removeAsk }) {
    const { row, main, side } = integrationRow(name, what);
    if (k.source === 'env') {
      side.append(badge(`••••${k.last4 || ''}`, 'ok'), badge(tr('settings.byEnv')));
      return row;
    }
    if (k.configured) side.append(badge(`••••${k.last4 || ''}`, 'ok'));
    else side.append(badge(tr('settings.notSet')));
    if (!storage) return row;

    const edit = el('form', 'cedar-set-inline');
    edit.hidden = true;
    const input = el('input');
    input.type = 'password';
    input.autocomplete = 'off';
    input.placeholder = tr('settings.apiKeyOf', { name });
    input.setAttribute('aria-label', tr('settings.apiKeyOf', { name }));
    const saveBtn = button(tr('settings.checkSave'), () => edit.requestSubmit(), 'primary');
    edit.append(input, saveBtn, button(tr('settings.cancel'), () => { edit.hidden = true; }));
    edit.addEventListener('submit', async (e) => {
      e.preventDefault();
      if (!input.value.trim()) return;
      saveBtn.disabled = true;
      saveBtn.textContent = tr('settings.checking');
      try {
        await adminApi('PUT', path, { api_key: input.value.trim() });
        deps.toast(saved);
        renderIntegrations();
        refreshTools();
      } catch (err) {
        saveBtn.disabled = false;
        saveBtn.textContent = tr('settings.checkSave');
        deps.toast(err.message);
      }
    });
    side.append(button(k.configured ? tr('settings.replace') : tr('settings.addKey'), () => { edit.hidden = false; input.focus(); }));
    if (k.configured) {
      side.append(button(tr('settings.remove'), async () => {
        if (!(await cedarConfirm(removeAsk, { ok: tr('settings.remove') }))) return;
        try {
          await adminApi('DELETE', path);
          renderIntegrations();
          refreshTools();
        } catch (err) { deps.toast(err.message); }
      }));
    }
    main.append(edit);
    return row;
  }

  // -- personal and community tools -----------------------------------------------------------------

  /** What a tool asks for, as short badges: hosts, secrets, packages, health data. */
  function permissionBadges(p = {}) {
    const out = [];
    if (p.health_data) out.push(badge(tr('settings.perm.health'), 'warn'));
    out.push(badge(p.network?.length ? tr('settings.perm.internet', { hosts: p.network.join(', ') }) : tr('settings.perm.noInternet')));
    if (p.secrets?.length) out.push(badge(tr('settings.perm.secrets', { count: p.secrets.length })));
    if (p.pip?.length) out.push(badge(tr('settings.perm.packages', { count: p.pip.length })));
    return out;
  }

  /** The tools in cedar's TOOLS_DIR, each with its secrets to fill in. `null` without TOOLS_DIR. */
  async function personalToolsBlock() {
    let info;
    try { info = await adminApi('GET', '/integrations/tools'); } catch (e) { return note(e.message, 'cedar-set-error'); }
    if (!info.tools_dir) return null;
    const wrap = el('div', 'cedar-set-help');
    if (!info.tools.length) {
      wrap.append(note(tr('settings.personal.none')));
      return wrap;
    }
    wrap.append(note(tr('settings.personal.intro')));
    for (const t of info.tools) wrap.append(personalToolRow(t, info.storage));
    return wrap;
  }

  function personalToolRow(t, storage) {
    const title = t.version ? `${t.name} ${t.version}` : t.name;
    const { row, main, side } = integrationRow(title, t.description);
    side.append(badge(t.source === 'marketplace' ? tr('settings.personal.community') : tr('settings.personal.local'), t.source === 'marketplace' ? '' : 'ok'));
    side.append(...permissionBadges({ network: t.network, secrets: t.secrets.map((x) => x.name), pip: t.pip, health_data: t.health_data }));
    if (t.author) main.append(el('small', '', tr('settings.by', { author: t.author })));
    if (t.health_data) main.append(note(tr('settings.personal.health'), 'cedar-set-warn-text cedar-set-note'));
    for (const secret of t.secrets) main.append(toolSecretRow(t.name, secret, storage));
    if (t.pip.length) {
      side.append(button(tr('settings.personal.reinstall'), async (e) => {
        const b = e.currentTarget;
        b.disabled = true;
        b.textContent = tr('settings.installing');
        try {
          await adminApi('POST', `/integrations/tools/${t.name}/setup`);
          deps.toast(tr('settings.personal.installed', { name: t.name }));
        } catch (err) { deps.toast(err.message); }
        b.disabled = false;
        b.textContent = tr('settings.personal.reinstall');
      }));
    }
    if (t.source === 'marketplace') {
      side.append(button(tr('settings.personal.uninstall'), async () => {
        if (!(await cedarConfirm(tr('settings.personal.confirmUninstall', { name: t.name }), { ok: tr('settings.personal.uninstall') }))) return;
        try {
          await adminApi('DELETE', `/integrations/marketplace/${t.name}`);
          deps.toast(tr('settings.personal.uninstalled', { name: t.name }));
          renderIntegrations();
          refreshTools();
        } catch (err) { deps.toast(err.message); }
      }));
    }
    return row;
  }

  /** One secret a tool declares: saved or not, with a form to set it. Values are never shown. */
  function toolSecretRow(tool, secret, storage) {
    const line = el('div', 'cedar-set-inline');
    const label = el('small', '', `${secret.optional ? tr('settings.secret.optional', { name: secret.name }) : secret.name}${secret.description ? ` — ${secret.description}` : ''}`);
    line.append(label, badge(secret.saved ? tr('settings.secret.saved') : tr('settings.notSet'), secret.saved ? 'ok' : ''));
    if (!storage) return line;
    const input = el('input');
    input.type = 'password';
    input.autocomplete = 'off';
    input.placeholder = secret.name;
    input.setAttribute('aria-label', `${tool} ${secret.name}`);
    input.hidden = true;
    const save = button(tr('settings.save'), async () => {
      if (!input.value) return;
      try {
        await adminApi('PUT', `/integrations/tools/${tool}/secrets/${secret.name}`, { value: input.value });
        deps.toast(tr('settings.secret.savedFor', { name: secret.name, tool }));
        renderIntegrations();
      } catch (err) { deps.toast(err.message); }
    }, 'primary');
    save.hidden = true;
    const edit = button(secret.saved ? tr('settings.replace') : tr('settings.secret.set'), () => { input.hidden = false; save.hidden = false; edit.hidden = true; input.focus(); });
    line.append(input, save, edit);
    if (secret.saved) {
      line.append(button(tr('settings.remove'), async () => {
        try {
          await adminApi('DELETE', `/integrations/tools/${tool}/secrets/${secret.name}`);
          renderIntegrations();
        } catch (err) { deps.toast(err.message); }
      }));
    }
    return line;
  }

  let lastRegistry = '';  // the registry's index URL, from the last marketplace listing

  /** The community registry's tools, to install or update. `null` when cedar has no registry. */
  async function marketplaceBlock(refresh = false) {
    let market;
    try {
      market = await adminApi('GET', `/integrations/marketplace${refresh ? '?refresh=true' : ''}`);
      lastRegistry = market.registry || '';
    } catch (e) {
      if (e.status === 409) return null;  // no TOOLS_REGISTRY_URL on cedar
      return note(e.message, 'cedar-set-error');
    }
    const wrap = el('div', 'cedar-set-help');
    wrap.append(note(tr('settings.market.intro')));
    if (!market.installable) wrap.append(note(tr('settings.market.noToolsDir'), 'cedar-set-error'));
    if (!market.tools.length) wrap.append(note(tr('settings.market.empty')));
    for (const l of market.tools) wrap.append(listingRow(l, market.installable));
    wrap.append(button(tr('settings.market.checkUpdates'), async () => {
      const fresh = await marketplaceBlock(true);
      if (fresh) wrap.replaceWith(fresh);
    }));
    return wrap;
  }

  function listingRow(l, installable) {
    const { row, main, side } = integrationRow(`${l.name} ${l.version}`, l.summary);
    const by = [l.author && tr('settings.by', { author: l.author }), l.category, l.license].filter(Boolean).join(' · ');
    if (by) main.append(el('small', '', by));
    side.append(...permissionBadges(l.permissions));
    if (l.reviewed) side.append(badge(tr('settings.market.reviewed'), 'ok'));
    if (l.conflict) {
      main.append(note(tr('settings.market.conflict', { reason: l.conflict }), 'cedar-set-note'));
      return row;
    }
    if (l.installed && !l.installed.update_available) {
      side.append(badge(tr('settings.market.installedVersion', { version: l.installed.version }), 'ok'));
      return row;
    }
    if (!installable) return row;
    const label = l.installed ? tr('settings.market.updateFrom', { version: l.installed.version }) : tr('settings.market.install');
    side.append(button(label, async (e) => {
      const b = e.currentTarget;  // null once the event is done, so before the confirmation
      if (!(await cedarConfirm(installQuestion(l), { ok: l.installed ? tr('settings.market.update') : tr('settings.market.install') }))) return;
      b.disabled = true;
      b.textContent = tr('settings.installing');
      try {
        await adminApi('POST', `/integrations/marketplace/${l.name}/install`, { accept: l.permissions });
        deps.toast(tr('settings.market.installed', { name: l.name, version: l.version }));
        renderIntegrations();
        refreshTools();
      } catch (err) {
        deps.toast(err.message);
        b.disabled = false;
        b.textContent = label;
      }
    }, 'primary'));
    return row;
  }

  /** What installing a tool lets it do, spelled out for the confirmation. */
  function installQuestion(l) {
    const p = l.permissions || {};
    const tool = `${l.name} ${l.version}`;
    const lines = [l.author ? tr('settings.install.askBy', { tool, author: l.author }) : tr('settings.install.ask', { tool }), '', tr('settings.install.able')];
    lines.push(`• ${p.network?.length ? tr('settings.install.reach', { hosts: p.network.join(', ') }) : tr('settings.install.offline')}`);
    if (p.secrets?.length) lines.push(`• ${tr('settings.install.secrets', { secrets: p.secrets.join(', ') })}`);
    if (p.pip?.length) lines.push(`• ${tr('settings.install.packages', { packages: p.pip.join(', ') })}`);
    lines.push(`• ${tr('settings.install.files')}`);
    if (p.health_data) {
      lines.push('', tr('settings.install.health'));
    }
    if (!l.reviewed) lines.push('', tr('settings.install.unreviewed'));
    return lines.join('\n');
  }

  /** cedar's max image size: attached images with a longer side are scaled down to it. */
  function imageSizeRow() {
    return serverSettingRow({
      title: tr('settings.imageSize.title'),
      desc: tr('settings.imageSize.desc'),
      key: 'max_image_side',
      unit: 'px',
      label: tr('settings.imageSize.label'),
      range: (d) => [d.min_max_image_side ?? 256, d.max_max_image_side ?? 8192],
      fallback: 1568,
      shown: (n) => `${n} px`,
      saved: (n) => tr('settings.imageSize.saved', { size: String(n) }),
    });
  }

  /** How long a conversation's sandbox keeps running after a turn before cedar pauses it. */
  function sandboxIdleRow() {
    const minutes = (n) => tr('settings.idle.minutes', { count: n });
    return serverSettingRow({
      title: tr('settings.idle.title'),
      desc: tr('settings.idle.desc'),
      key: 'sandbox_idle_minutes',
      unit: 'min',
      label: tr('settings.idle.label'),
      range: (d) => [0, d.max_sandbox_idle_minutes ?? 1440],
      fallback: 10,
      shown: (n) => (n === 0 ? tr('settings.idle.now') : minutes(n)),
      saved: (n) => (n === 0 ? tr('settings.idle.savedNow') : tr('settings.idle.saved', { minutes: minutes(n) })),
    });
  }

  /** A number in cedar's server-wide settings (`/integrations/settings`), saved on its own: the
   * other settings are left out of the request, so they keep their values. Empty is the default. */
  async function serverSettingRow(o) {
    const { row, main, side } = integrationRow(o.title, o.desc);
    let settings;
    try {
      settings = await adminApi('GET', '/integrations/settings');
    } catch (err) {
      side.append(badge(tr('settings.unavailable')));
      main.append(el('small', '', err.message));
      return row;
    }
    const d = settings.defaults || {};
    const fallback = d[o.key] ?? o.fallback;
    const [min, max] = o.range(d);
    const form = el('form', 'cedar-set-inline');
    const input = el('input');
    input.type = 'number';
    input.min = String(min);
    input.max = String(max);
    input.step = '1';
    input.placeholder = tr('settings.defaultValue', { value: String(fallback) });
    input.value = settings[o.key] ?? '';
    input.setAttribute('aria-label', o.label);
    const saveBtn = button(tr('settings.save'), () => form.requestSubmit(), 'primary');
    form.append(input, el('span', '', o.unit), saveBtn);
    form.addEventListener('submit', async (e) => {
      e.preventDefault();
      if (!input.reportValidity()) return;
      saveBtn.disabled = true;
      try {
        const saved = await adminApi('PUT', '/integrations/settings', { [o.key]: input.value ? Number(input.value) : null });
        deps.toast(o.saved(saved[o.key] ?? fallback));
        renderIntegrations();
      } catch (err) {
        saveBtn.disabled = false;
        deps.toast(err.message);
      }
    });
    const value = settings[o.key];
    side.append(badge(value != null ? o.shown(value) : tr('settings.models.default'), 'ok'));
    main.append(form);
    return row;
  }

  function googleRow(status) {
    const g = status.google || {};
    const names = (g.services || []).map((s) => s[0].toUpperCase() + s.slice(1));
    const services = new Intl.ListFormat(cedarI18n.locale, { type: 'conjunction' }).format(names.length ? names : ['Gmail', 'Calendar']);
    const { row, main, side } = integrationRow(tr('settings.google.title'), tr('settings.google.desc', { services }));
    if (!g.enabled) {
      side.append(badge(tr('settings.off')));
      main.append(el('small', '', tr('settings.google.disabled')));
      return row;
    }
    if (g.source === 'env') {
      side.append(badge(tr('settings.google.connected'), 'ok'), badge(tr('settings.byEnv')));
      return row;
    }
    if (g.configured) side.append(badge(g.account || tr('settings.google.connected'), 'ok'));
    else side.append(badge(tr('settings.google.notConnected')));
    if (!status.storage) return row;
    if (g.configured) {
      side.append(button(tr('settings.google.disconnect'), async () => {
        if (!(await cedarConfirm(tr('settings.google.confirmDisconnect'), { ok: tr('settings.google.disconnect') }))) return;
        try {
          await adminApi('DELETE', '/integrations/google');
          renderIntegrations();
          refreshTools();
        } catch (err) { deps.toast(err.message); }
      }));
    }
    side.append(button(g.configured ? tr('settings.google.reconnect') : tr('settings.google.connect'), async () => {
      try {
        const { url } = await adminApi('POST', '/integrations/google/connect');
        location.assign(url);
      } catch (err) { deps.toast(err.message); }
    }, g.configured ? 'copy-btn' : 'primary'));
    if (g.redirect_uri) {
      const hint = el('small', 'cedar-set-hint');
      const [before, after] = tr('settings.google.redirect').split('{uri}');
      hint.append(before, el('code', '', g.redirect_uri), after ?? '');
      main.append(hint);
    }
    return row;
  }

  async function refreshTools() {
    try { await loadCatalog(true); } catch { /* the next render retries */ }
    if (!document.getElementById('settings-dialog')?.open) return;
    if (toolsSection) renderTools(toolsSection);
    if (modelSection) renderModel(modelSection);
    shell()?.refresh();
  }

  // -- the composer's Tools button ---------------------------------------------------------------

  /** The open conversation's own tools, a new chat's pending choice, or undefined for the defaults. */
  function override() {
    const c = deps.current();
    if (c) return c.tools ?? undefined;
    return pending;
  }

  function updateChip() {
    if (!chip) return;
    const own = override();
    const differs = own !== undefined && prefs && !sameSelection(own, prefs.tools);
    chip.classList.toggle('custom', own !== undefined && (prefs ? differs : true));
    chip.title = own === undefined ? tr('settings.chip.defaults') : tr('settings.chip.custom');
  }

  async function openMenu() {
    try {
      await loadCatalog();
      if (!prefs) prefs = await deps.api('GET', '/settings');
    } catch (e) {
      deps.toast(e.message);
      return;
    }
    if (!menu) {
      menu = el('dialog', 'cedar-set-menu');
      menu.setAttribute('aria-label', tr('settings.chip.menuTitle'));
      document.body.append(menu);
    }
    const own = override();
    let state = decode(own === undefined ? prefs.tools : own);
    let chosen = own;
    const apply = button(tr('settings.chip.apply'), () => save(chosen), 'primary');
    const head = el('div', 'cedar-set-menu-head');
    head.append(el('h3', '', tr('settings.chip.menuTitle')), button(tr('settings.close'), () => menu.close()));
    const picker = toolPicker(state, (s) => { state = s; chosen = encode(s); }, { detailed: false });
    const actions = el('div', 'cedar-set-actions');
    actions.append(button(tr('settings.chip.useDefaults'), () => save(undefined)), apply);
    menu.replaceChildren(head, note(tr('settings.chip.alwaysOn')), picker, actions);
    menu.showModal();

    async function save(tools) {
      const c = deps.current();
      if (!c) {
        // A new chat: applied when the conversation is created (see beforeSend).
        pending = tools === undefined ? undefined : tools ?? [ALL];
        menu.close();
        updateChip();
        return;
      }
      try {
        const body = { tools: tools === undefined ? null : tools ?? [ALL] };
        const updated = await deps.api('PUT', `/conversations/${c.id}/tools`, body);
        c.tools = updated.tools;
        menu.close();
        updateChip();
      } catch (e) {
        deps.toast(e.message);
      }
    }
  }

  /** Call right after a new chat's conversation is created, before its first message. */
  async function beforeSend(conversation) {
    if (pending === undefined) return;
    const tools = pending;
    pending = undefined;
    const updated = await deps.api('PUT', `/conversations/${conversation.id}/tools`, { tools });
    conversation.tools = updated.tools;
    updateChip();
  }

  /** Call when another conversation (or a new chat) is opened. */
  function conversationChanged() {
    if (deps.current()) pending = undefined;
    updateChip();
  }

  // -- wiring ------------------------------------------------------------------------------------

  /** After "Connect Google", Google's redirect lands on /?integrations=…: report it and reopen Settings. */
  function reportRedirect() {
    const params = new URLSearchParams(location.search);
    const outcome = params.get('integrations');
    if (!outcome) return;
    history.replaceState(null, '', location.pathname + location.hash);
    deps.toast(outcome === 'connected' ? tr('settings.google.connectedToast') : `Google: ${params.get('message') || tr('settings.google.notConnectedLower')}`);
    const btn = document.getElementById('settings-btn');
    if (btn) { btn.dataset.page = 'server'; btn.click(); }
  }

  function bind() {
    integrationsSection = document.getElementById('integrations-section');
    // The pages of the Settings dialog; settings-shell.js opens it and calls render each time.
    const s = shell();
    if (s) {
      s.register({
        id: 'tools', label: tr('settings.tools.title'), title: tr('settings.tools.title'),
        description: tr('settings.tools.description'),
        render: renderTools, summary: toolsSummary, save: submit, footer: true,
      });
      s.register({
        id: 'model', label: tr('settings.model.title'), title: tr('settings.model.title'),
        description: tr('settings.model.description'),
        render: renderModel, summary: modelSummary, save: submit, footer: true,
      });
      s.register({
        id: 'server', label: tr('settings.server.title'), title: tr('settings.server.title'),
        description: tr('settings.server.description'),
        render: renderIntegrations, summary: () => null,
        tag: () => (deps?.config()?.admin_enabled && unlocked() ? 'UNLOCKED' : 'ADMIN'),
        actions: serverActions, footer: false,
      });
      s.register({
        id: 'advanced', label: tr('settings.advanced.title'), title: tr('settings.advanced.title'),
        description: tr('settings.advanced.description'),
        render: renderAdvanced, summary: advancedSummary, footer: false,
      });
    }
    if (deps.anchor) {
      chip = button(tr('settings.tools.title'), openMenu, 'copy-btn cedar-set-chip');
      chip.setAttribute('aria-label', tr('settings.chip.menuTitle'));
      deps.anchor.after(chip);
      updateChip();
    }
  }

  function init(helpers) {
    deps = helpers;
    if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', bind);
    else bind();
  }

  /** Call once the app has its config and access token: loads the defaults and reports the
   *  outcome of a "Connect Google" redirect. */
  function start() {
    deps.api('GET', '/settings').then((p) => { prefs = p; updateChip(); }).catch(() => {});
    reportRedirect();
  }

  return { init, start, beforeSend, conversationChanged };
})();

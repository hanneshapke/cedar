'use strict';

// Settings that change what the agent can do, shared by every UI. Three pieces, the first two
// pages of the Settings dialog that settings-shell.js frames:
//
//   - The "Tools" and "Model & prompt" pages: which of cedar's tool groups new conversations
//     get, the default model, turn limit and system prompt (GET/PUT /api/settings). Both edit one
//     draft and save it through the dialog's footer.
//   - The "Server" page, for admins only (CHAT_ADMIN_TOKEN): the models cedar has registered
//     (each with its server, API key and reasoning levels), its Brave Search key and its Google
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
      const err = new Error(data?.error?.message || `Request failed (${res.status})`);
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
    return cedarConfirm(`These tools can change things, not just read them:\n\n${names.join('\n')}\n\nLet the agent use them?`, { ok: 'Allow' });
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
        ? (g.tools?.length ? g.tools.map((t) => t.name).join(', ') : 'Tools appear once cedar connects')
        : g.reason || 'Not offered by this server';
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
        details.append(el('summary', '', 'Choose single tools'));
        const inner = el('div', 'cedar-set-single-list');
        for (const t of g.tools) {
          const l = el('label', 'cedar-set-check small');
          const b = el('input');
          b.type = 'checkbox';
          b.checked = v === 'all' || (v instanceof Set && v.has(t.name));
          l.append(b, el('span', '', t.name));
          if (t.writes) l.append(el('span', 'cedar-set-warn', 'changes things'));
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
        row.append(el('span', 'cedar-set-warn', 'includes tools that change things'));
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
    section.replaceChildren(note('Loading…'));
    try { await loadPrefs(); } catch (e) {
      section.replaceChildren(note(`Could not load the settings: ${e.message}`, 'cedar-set-error'));
      return;
    }
    const list = el('div', 'cedar-set-tools');
    const always = el('div', 'cedar-set-always');
    always.append(el('span', 'settings-eyebrow', 'Always on'), el('span', '', 'bash, files, Python, documents, PDF, charts'));
    list.append(always);
    const changed = () => {
      draft.tools = encode(toolState);
      shell()?.setDirty('tools', toolChanges());
    };
    for (const g of groups()) list.append(toolRow(g, changed));
    section.replaceChildren(list);
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
      side.textContent = g.reason || 'Not offered by this server';
    } else if (single) {
      sub.append(`${g.tools.length} tools · `);
      const choose = el('button', 'settings-link', 'choose single tools');
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
        if (t.writes) l.append(el('span', 'cedar-set-warn', 'changes things'));
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
      sub.textContent = g.tools?.length ? g.tools.map((t) => t.name).join(', ') : 'Tools appear once cedar connects';
    }
    if (g.available && g.tools?.some((t) => t.writes)) {
      side.textContent = 'Can change things';
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
    section.replaceChildren(note('Loading…'));
    try { await loadPrefs(); } catch (e) {
      section.replaceChildren(note(`Could not load the settings: ${e.message}`, 'cedar-set-error'));
      return;
    }
    const defaults = prefs.defaults || null;
    const dirty = () => shell()?.setDirty('model', true);
    const parts = [];

    const fields = el('div', 'cedar-set-fields');
    if (cedarModels.all().length) {
      const modelSelect = el('select');
      const def = cedarModels.byId(cedarModels.defaultId());
      modelSelect.append(new Option(def ? `cedar's default (${modelLabel(def)})` : "cedar's default", ''));
      for (const m of cedarModels.all()) modelSelect.append(new Option(modelLabel(m), m.id));
      modelSelect.value = cedarModels.byId(draft.model) ? draft.model : '';
      modelSelect.addEventListener('change', () => { draft.model = modelSelect.value || null; dirty(); });
      fields.append(field('Default model', modelSelect));
    } else if (cedarModels.loaded()) {
      parts.push(note('No model is registered yet: an admin adds one on the Server page.', 'cedar-set-error'));
    }
    const turns = el('input');
    turns.type = 'number';
    turns.min = '1';
    turns.max = '1000';
    turns.placeholder = defaults ? `Server limit (${defaults.max_turns_cap})` : 'Server limit';
    turns.value = draft.max_turns ?? '';
    turns.addEventListener('input', () => { draft.max_turns = turns.value ? Number(turns.value) : null; dirty(); });
    fields.append(field('Max turns per message', turns));
    const plan = el('select');
    for (const [value, label] of [['auto', 'For longer requests'], ['on', 'Always'], ['off', 'Never']]) plan.append(new Option(label, value));
    plan.value = draft.plan || 'auto';
    plan.addEventListener('change', () => { draft.plan = plan.value; dirty(); });
    fields.append(field('Plan before starting', plan));
    parts.push(fields);
    const notes = el('div', 'cedar-set-field-notes');
    notes.append(el('div', 'cedar-set-field-note', 'Plan before starting: Cedar proposes its steps first and waits until you start it; you can edit them.'));
    parts.push(notes);

    // The system prompt: read as a document, edited as one. Without a saved prompt it shows
    // cedar's default text, which is saved as "unset" so it keeps following cedar's SYSTEM_PROMPT.
    const block = el('div', 'cedar-set-prompt-block');
    const head = el('div', 'cedar-set-prompt-head');
    const headLeft = el('span');
    const tag = el('span', 'cedar-set-badge', "cedar's default");
    headLeft.append(el('span', 'settings-eyebrow', 'System prompt'), tag);
    const acts = el('div', 'cedar-set-prompt-acts');
    head.append(headLeft, acts);
    const reading = el('div', 'cedar-set-prompt');
    const editor = el('textarea', 'cedar-set-prompt-edit');
    editor.setAttribute('aria-label', 'System prompt');
    editor.placeholder = defaults ? '' : "cedar's default system prompt (cedar could not be asked for its text)";
    const text = () => draft.system ?? defaultSystem();
    const sync = () => {
      tag.hidden = !isDefaultPrompt();
      reading.textContent = text();
      editor.value = text();
      reading.hidden = editingPrompt;
      editor.hidden = !editingPrompt;
      acts.textContent = '';
      if (editingPrompt) {
        if (!isDefaultPrompt()) acts.append(button('Use the default', () => { draft.system = null; editingPrompt = false; sync(); dirty(); }, 'settings-ghost'));
        acts.append(button('Done', () => { editingPrompt = false; sync(); }, 'settings-ghost'));
      } else {
        acts.append(button('Edit', () => { editingPrompt = true; sync(); editor.focus(); }, 'settings-ghost'));
      }
    };
    editor.addEventListener('input', () => {
      draft.system = editor.value;
      tag.hidden = !isDefaultPrompt();
      dirty();
    });
    sync();
    block.append(head, reading, editor, el('div', 'cedar-set-field-note', 'cedar adds a short note after it saying which tools this conversation has and what the sandbox can reach.'));
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
    if (prefs.tools === null || prefs.tools === undefined) return 'Everything';
    const state = decode(prefs.tools);
    const on = [...state.values()].filter((v) => v !== 'none').length;
    return `${on} of ${state.size} groups on`;
  }

  function modelSummary() {
    if (!prefs) return null;
    if (cedarModels.loaded() && !cedarModels.all().length) return { text: 'No model registered', kind: 'error' };
    const m = cedarModels.byId(prefs.model) || cedarModels.byId(cedarModels.defaultId());
    const parts = [];
    if (m) parts.push(m.label || m.model || m.id);
    if (m?.default_reasoning) parts.push(m.default_reasoning);
    parts.push(prefs.system ? 'custom prompt' : 'default prompt');
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
    section.replaceChildren(note('Loading…'));
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
      parts.push(note('cedar has no CEDAR_SECRET_KEY, so API keys can\'t be saved here: models whose server needs none can still be registered; the Brave key and the Google account can only be set in cedar\'s environment.'));
    }
    const block = (title, ...children) => {
      const b = el('div', 'cedar-set-block');
      const head = el('div', 'cedar-set-block-head');
      head.append(el('span', 'settings-eyebrow', title));
      b.append(head, ...children);
      return b;
    };
    const models = await modelsBlock(status);
    const modelsBlockEl = block('Models', models.wrap);
    modelsBlockEl.querySelector('.cedar-set-block-head').append(models.add);
    parts.push(modelsBlockEl, block('Brave Search', braveRow(status)), block('Google account', googleRow(status)));
    section.replaceChildren(...parts);
  }

  /** The page head's Lock button, when the admin token is in. */
  function serverActions() {
    if (!deps?.config()?.admin_enabled || !unlocked()) return null;
    return [button('Lock', () => { setAdminToken(''); renderIntegrations(); shell()?.refresh('server'); }, 'settings-ghost')];
  }

  /** Where keys go when this page can't manage them (no CHAT_ADMIN_TOKEN). */
  function setupHelp() {
    const wrap = el('div', 'cedar-set-help');
    wrap.append(note('The models (each with its server and API key), the Brave Search key and the Google account are set on the cedar server. They can be managed here once these are set:'));
    const list = el('ul');
    const item = (code, text) => {
      const li = el('li');
      li.append(el('code', '', code), ` ${text}`);
      list.append(li);
    };
    item('CHAT_ADMIN_TOKEN', 'on cedar-chat: the token that unlocks this section');
    item('CEDAR_SECRET_KEY', 'on cedar (openssl rand -hex 32): encrypts the API keys saved here');
    item('CEDAR_ADMIN_KEYS', "on cedar: include cedar-chat's CEDAR_API_KEY");
    wrap.append(list);
    wrap.append(note('Models are only registered through cedar\'s admin API (PUT /v1/admin/models/{id}) or here. BRAVE_API_KEY and the GOOGLE_* variables can also go in cedar\'s environment (see .env.example); everything else, like the sandbox and limits, is only set there.'));
    if (cedarModels.loaded() && !cedarModels.all().length) {
      wrap.append(note('No model is registered yet, so messages can\'t be sent.', 'cedar-set-error'));
    }
    const missing = groups().filter((g) => !g.available);
    if (missing.length) {
      const l = el('ul');
      for (const g of missing) {
        const li = el('li');
        li.append(el('strong', '', g.label), `: ${g.reason || 'not offered'}`);
        l.append(li);
      }
      wrap.append(note('Not available on this server right now:'), l);
    }
    return wrap;
  }

  function unlockForm(error) {
    const form = el('form', 'cedar-set-unlock');
    const input = el('input');
    input.type = 'password';
    input.autocomplete = 'off';
    input.placeholder = 'Admin token';
    input.setAttribute('aria-label', 'Admin token');
    form.append(input, button('Unlock', () => form.requestSubmit(), 'primary'));
    form.addEventListener('submit', (e) => {
      e.preventDefault();
      if (!input.value.trim()) return;
      setAdminToken(input.value.trim());
      renderIntegrations();
      refreshTools();
    });
    const wrap = el('div', 'cedar-set-unlock-card');
    wrap.append(note('The models, API keys and the Google account are for admins. Enter the admin token (CHAT_ADMIN_TOKEN) to manage them.'));
    if (cedarModels.loaded() && !cedarModels.all().length) {
      wrap.append(note('No model is registered yet, so messages can\'t be sent until an admin adds one here.', 'cedar-set-error'));
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
      wrap.append(note(`Could not list the models: ${e.message}`, 'cedar-set-error'));
      return { wrap, add: el('span') };
    }
    entries = entries || [];
    // The button sits in the block's heading, so it is in view however long the list is. The form
    // it opens goes right under the intro, above the first entry.
    const intro = entries.length
      ? note('Each model is served by its own OpenAI-compatible server. New chats use the default; the picker by the message box chooses another, and the reasoning level next to it. Add a model for another server, or another model on the same one.')
      : note('No model is registered yet, so messages can\'t be sent. Add the server that runs your model: for a local vLLM, http://localhost:8000/v1 and the name it registered (curl localhost:8000/v1/models).', 'cedar-set-error');
    wrap.append(intro);
    const add = button('Add a model', () => {
      add.hidden = true;
      const form = modelForm(null, status, () => { add.hidden = false; form.remove(); });
      intro.after(form);
      form.querySelector('input').focus();
    }, entries.length ? 'settings-ghost' : 'primary');
    add.setAttribute('aria-label', 'Add a model');
    for (const entry of entries) wrap.append(modelRow(entry, status));
    return { wrap, add };
  }

  function modelRow(entry, status) {
    const levels = entry.reasoning_levels || [];
    const { row, main, side } = integrationRow(entry.label || entry.id, `${entry.model} at ${entry.base_url}`);
    main.append(el('small', '', `id: ${entry.id}`));
    if (entry.is_default) side.append(badge('Default', 'ok'));
    if (entry.api_key_set) side.append(badge('API key set'));
    if (entry.multimodal) side.append(badge('Images'));
    side.append(badge(levels.length ? `Reasoning: ${levels.join(', ')}${entry.default_reasoning ? ` (default ${entry.default_reasoning})` : ''}` : 'No reasoning levels'));
    if (entry.max_tokens) side.append(badge(`Max ${entry.max_tokens.toLocaleString()} tokens`));
    // What the server listed when the entry was checked is offered in the edit form; the row only
    // says when the model name is not one of them.
    if (Array.isArray(entry.listed_models) && !entry.listed_models.includes(entry.model)) {
      const listed = entry.listed_models.length ? entry.listed_models.join(', ') : 'none';
      main.append(el('small', 'cedar-set-warn-text', `"${entry.model}" is not a model this server lists (${listed}); use Edit to pick one it does.`));
    }
    let form = null;
    side.append(button('Edit', () => {
      if (form) { form.remove(); form = null; return; }
      form = modelForm(entry, status, () => { form?.remove(); form = null; });
      row.append(form);
      form.querySelector('input').focus();
    }));
    if (!entry.is_default) {
      side.append(button('Make default', async () => {
        try {
          await adminApi('PUT', `/integrations/models/${encodeURIComponent(entry.id)}`, { base_url: entry.base_url, model: entry.model, is_default: true });
          deps.toast(`${entry.label || entry.id} is the default model`);
          await modelsChanged();
          renderIntegrations();
        } catch (err) { deps.toast(err.message); }
      }));
    }
    side.append(button('Remove', async () => {
      if (!(await cedarConfirm(`Remove the model ${entry.label || entry.id}? Conversations using it fail at their next message until another model is picked.`, { ok: 'Remove' }))) return;
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
    const id = input('id (letters, digits, . _ -), like deepseek', entry?.id || '', 'Model id');
    id.disabled = Boolean(entry);
    const label = input('Name shown in the picker', entry?.label || '', 'Label');
    let idTouched = Boolean(entry);
    id.addEventListener('input', () => { idTouched = true; });
    label.addEventListener('input', () => { if (!idTouched) id.value = slug(label.value); });
    // The whole address, protocol and path included; cedar adds /v1 if the path is left off.
    const address = input('Server address, like http://10.0.0.5:8000/v1', entry?.base_url || '', 'Server address');
    const canSaveKey = Boolean(status.storage);
    const key = input(
      !canSaveKey ? 'API key (needs CEDAR_SECRET_KEY on cedar)' : entry?.api_key_set ? 'API key (leave empty to keep it)' : 'API key, if the server needs one',
      '', 'API key', 'password',
    );
    key.autocomplete = 'off';
    key.disabled = !canSaveKey;
    const clearKey = el('label', 'cedar-set-check small plain');
    const clearBox = el('input');
    clearBox.type = 'checkbox';
    clearKey.append(clearBox, el('span', '', 'Remove the saved API key'));
    // The model name is picked from what the server lists: "List models" asks it (with the key
    // typed above, else the entry's saved one), and the picker offers those names only. An entry
    // being edited starts with what its server listed when it was last checked.
    const model = el('select');
    model.setAttribute('aria-label', 'Model name');
    const listBtn = button('List models', () => listServerModels());
    const modelRow = el('div', 'cedar-set-server-models');
    modelRow.append(model, listBtn);
    const fillModels = (names, selected) => {
      model.textContent = '';
      if (!names) {
        model.append(new Option('Enter the address, then list its models', ''));
        model.disabled = true;
        return;
      }
      model.disabled = false;
      const options = [...names];
      if (selected && !options.includes(selected)) options.unshift(selected);
      model.append(new Option(options.length ? 'Choose a model' : 'The server lists no models', ''));
      for (const m of options) model.append(new Option(m === selected && !names.includes(m) ? `${m} (not listed by the server)` : m, m));
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
      if (!address.value.trim()) { deps.toast('Enter the server address'); address.focus(); return; }
      const body = { base_url: address.value.trim() };
      if (key.value.trim()) body.api_key = key.value.trim();
      else if (entry && !clearBox.checked) body.id = entry.id;
      const previous = model.value || entry?.model || '';
      listBtn.disabled = true;
      listBtn.textContent = 'Listing…';
      try {
        const found = await adminApi('POST', '/integrations/server-models', body);
        fillModels(found.models || [], found.models?.includes(previous) ? previous : '');
        model.dispatchEvent(new Event('change'));
        if (!found.models?.length) deps.toast('The server lists no models');
        model.focus();
      } catch (err) {
        fillModels(null);
        deps.toast(err.message);
      } finally {
        listBtn.disabled = false;
        listBtn.textContent = 'List models';
      }
    }
    const levels = input('Reasoning levels the model takes, like none, low, high, max (empty: none)', (entry?.reasoning_levels || []).join(', '), 'Reasoning levels');
    const defaultLevel = el('select');
    defaultLevel.setAttribute('aria-label', 'Default reasoning level');
    const parseLevels = () => [...new Set(levels.value.split(',').map((l) => l.trim()).filter(Boolean))];
    const fillDefault = () => {
      const current = defaultLevel.value || entry?.default_reasoning || '';
      defaultLevel.textContent = '';
      defaultLevel.append(new Option("Default level: the server's own", ''));
      for (const l of parseLevels()) defaultLevel.append(new Option(`Default level: ${l}`, l));
      defaultLevel.value = parseLevels().includes(current) ? current : '';
      defaultLevel.hidden = !parseLevels().length;
    };
    levels.addEventListener('input', fillDefault);
    fillDefault();
    const maxTokens = input('Max tokens per reply', String(entry?.max_tokens ?? DEFAULT_MAX_TOKENS), 'Max tokens per reply', 'number');
    maxTokens.min = '1';
    maxTokens.max = String(MAX_MAX_TOKENS);
    maxTokens.step = '1';
    const maxTokensField = el('label', 'cedar-set-field');
    const maxTokensHead = el('span', 'cedar-set-field-head');
    maxTokensHead.append(el('span', '', 'Max tokens per reply'));
    maxTokensField.append(maxTokensHead, maxTokens, el('small', 'cedar-set-field-note', 'The most one reply may take, a reasoning model\'s thinking included. A reply that reaches it is cut off.'));
    const multimodalLabel = el('label', 'cedar-set-check plain');
    const multimodalBox = el('input');
    multimodalBox.type = 'checkbox';
    multimodalBox.checked = Boolean(entry?.multimodal);
    const multimodalText = el('span');
    multimodalText.append(el('strong', '', 'Accepts images'));
    multimodalText.append(el('small', '', 'Lets images be attached to a message while this model is picked, and shows it the first pages of attached PDFs as images (PDFs can be attached for any model; the agent reads their text). cedar passes images to the model as they are; a model that cannot read them fails the message.'));
    multimodalLabel.append(multimodalBox, multimodalText);
    const makeDefault = el('label', 'cedar-set-check small plain');
    const defaultBox = el('input');
    defaultBox.type = 'checkbox';
    defaultBox.checked = Boolean(entry?.is_default);
    defaultBox.disabled = Boolean(entry?.is_default);
    makeDefault.append(defaultBox, el('span', '', 'Use as the default model for new chats'));
    const saveBtn = button(entry ? 'Check and save' : 'Check and add', () => form.requestSubmit(), 'primary');
    form.append(id, label, address, key);
    if (entry?.api_key_set && canSaveKey) form.append(clearKey);
    form.append(modelRow);
    form.append(levels, defaultLevel, maxTokensField, multimodalLabel, makeDefault);
    form.append(el('small', 'cedar-set-field-note', 'cedar checks the address again by listing the server\'s models before saving. DeepSeek V4 on vLLM takes the levels none, low, high, xhigh and max as reasoning_effort; OpenAI-style servers take none, minimal, low, medium and high.'));
    form.append(saveBtn, button('Cancel', onDone));
    form.addEventListener('submit', async (e) => {
      e.preventDefault();
      if (!id.value.trim()) { deps.toast('Give the model an id'); id.focus(); return; }
      if (!address.value.trim()) { deps.toast('Enter the server address'); address.focus(); return; }
      if (model.disabled) { deps.toast('List the server\'s models, then choose one'); listBtn.focus(); return; }
      if (!model.value) { deps.toast('Choose a model'); model.focus(); return; }
      const limit = Number(maxTokens.value);
      if (!Number.isInteger(limit) || limit < 1 || limit > MAX_MAX_TOKENS) {
        deps.toast(`Max tokens per reply must be a whole number from 1 to ${MAX_MAX_TOKENS.toLocaleString()}`);
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
      saveBtn.textContent = 'Checking…';
      try {
        const saved = await adminApi('PUT', `/integrations/models/${encodeURIComponent(id.value.trim())}`, body);
        deps.toast(`${saved.label || saved.id}: ${saved.model} at ${saved.base_url}`);
        await modelsChanged();
        renderIntegrations();
        refreshTools();
      } catch (err) {
        saveBtn.disabled = false;
        saveBtn.textContent = entry ? 'Check and save' : 'Check and add';
        deps.toast(err.message);
      }
    });
    return form;
  }

  function braveRow(status) {
    const b = status.brave || {};
    const { row, main, side } = integrationRow('Brave Search', 'Turns on web_search. The key stays on the cedar server.');
    if (b.source === 'env') {
      side.append(badge(`••••${b.last4 || ''}`, 'ok'), badge('Managed by environment'));
      return row;
    }
    if (b.configured) side.append(badge(`••••${b.last4 || ''}`, 'ok'));
    else side.append(badge('Not set'));
    if (!status.storage) return row;

    const edit = el('form', 'cedar-set-inline');
    edit.hidden = true;
    const input = el('input');
    input.type = 'password';
    input.autocomplete = 'off';
    input.placeholder = 'Brave Search API key';
    input.setAttribute('aria-label', 'Brave Search API key');
    const saveBtn = button('Check and save', () => edit.requestSubmit(), 'primary');
    edit.append(input, saveBtn, button('Cancel', () => { edit.hidden = true; }));
    edit.addEventListener('submit', async (e) => {
      e.preventDefault();
      if (!input.value.trim()) return;
      saveBtn.disabled = true;
      saveBtn.textContent = 'Checking…';
      try {
        await adminApi('PUT', '/integrations/brave', { api_key: input.value.trim() });
        deps.toast('Brave Search key saved; web search is on');
        renderIntegrations();
        refreshTools();
      } catch (err) {
        saveBtn.disabled = false;
        saveBtn.textContent = 'Check and save';
        deps.toast(err.message);
      }
    });
    side.append(button(b.configured ? 'Replace' : 'Add key', () => { edit.hidden = false; input.focus(); }));
    if (b.configured) {
      side.append(button('Remove', async () => {
        if (!(await cedarConfirm('Remove the Brave Search key? Web search stops working for new messages.', { ok: 'Remove' }))) return;
        try {
          await adminApi('DELETE', '/integrations/brave');
          renderIntegrations();
          refreshTools();
        } catch (err) { deps.toast(err.message); }
      }));
    }
    main.append(edit);
    return row;
  }

  function googleRow(status) {
    const g = status.google || {};
    const services = (g.services || []).map((s) => s[0].toUpperCase() + s.slice(1)).join(' and ') || 'Gmail and Calendar';
    const { row, main, side } = integrationRow('Google account', `For the ${services} tools. The token stays on the cedar server.`);
    if (!g.enabled) {
      side.append(badge('Off'));
      main.append(el('small', '', 'Turn on GMAIL or CALENDAR and set GOOGLE_CLIENT_ID and GOOGLE_CLIENT_SECRET on cedar first.'));
      return row;
    }
    if (g.source === 'env') {
      side.append(badge('Connected', 'ok'), badge('Managed by environment'));
      return row;
    }
    if (g.configured) side.append(badge(g.account || 'Connected', 'ok'));
    else side.append(badge('Not connected'));
    if (!status.storage) return row;
    if (g.configured) {
      side.append(button('Disconnect', async () => {
        if (!(await cedarConfirm('Disconnect the Google account? Its tools stop working for new messages.', { ok: 'Disconnect' }))) return;
        try {
          await adminApi('DELETE', '/integrations/google');
          renderIntegrations();
          refreshTools();
        } catch (err) { deps.toast(err.message); }
      }));
    }
    side.append(button(g.configured ? 'Reconnect' : 'Connect Google', async () => {
      try {
        const { url } = await adminApi('POST', '/integrations/google/connect');
        location.assign(url);
      } catch (err) { deps.toast(err.message); }
    }, g.configured ? 'copy-btn' : 'primary'));
    if (g.redirect_uri) {
      const hint = el('small', 'cedar-set-hint');
      hint.append('Add ', el('code', '', g.redirect_uri), ' as an authorized redirect URI of the OAuth client.');
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
    chip.title = own === undefined ? 'Tools: the defaults from Settings' : 'Tools: chosen for this conversation';
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
      menu.setAttribute('aria-label', 'Tools for this conversation');
      document.body.append(menu);
    }
    const own = override();
    let state = decode(own === undefined ? prefs.tools : own);
    let chosen = own;
    const apply = button('Apply', () => save(chosen), 'primary');
    const head = el('div', 'cedar-set-menu-head');
    head.append(el('h3', '', 'Tools for this conversation'), button('Close', () => menu.close()));
    const picker = toolPicker(state, (s) => { state = s; chosen = encode(s); }, { detailed: false });
    const actions = el('div', 'cedar-set-actions');
    actions.append(button('Use the defaults', () => save(undefined)), apply);
    menu.replaceChildren(head, note('Sandbox tools (files, shell, Python, documents, charts) are always on.'), picker, actions);
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
    deps.toast(outcome === 'connected' ? 'Google account connected' : `Google: ${params.get('message') || 'not connected'}`);
    const btn = document.getElementById('settings-btn');
    if (btn) { btn.dataset.page = 'server'; btn.click(); }
  }

  function bind() {
    integrationsSection = document.getElementById('integrations-section');
    // The pages of the Settings dialog; settings-shell.js opens it and calls render each time.
    const s = shell();
    if (s) {
      s.register({
        id: 'tools', label: 'Tools', title: 'Tools',
        description: 'What every new conversation may use. The Tools button by the message box changes it for one conversation.',
        render: renderTools, summary: toolsSummary, save: submit, footer: true,
      });
      s.register({
        id: 'model', label: 'Model & prompt', title: 'Model & prompt',
        description: 'Defaults for every new conversation. The picker by the message box changes the model for one.',
        render: renderModel, summary: modelSummary, save: submit, footer: true,
      });
      s.register({
        id: 'server', label: 'Server', title: 'Server',
        description: 'Models, API keys and the Google account on the cedar server.',
        render: renderIntegrations, summary: () => null,
        tag: () => (deps?.config()?.admin_enabled && unlocked() ? 'UNLOCKED' : 'ADMIN'),
        actions: serverActions, footer: false,
      });
    }
    if (deps.anchor) {
      chip = button('Tools', openMenu, 'copy-btn cedar-set-chip');
      chip.setAttribute('aria-label', 'Tools for this conversation');
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

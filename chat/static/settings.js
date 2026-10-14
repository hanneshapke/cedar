'use strict';

// Settings that change what the agent can do, shared by both UIs (/v1 and /v2). Three pieces:
//
//   - The "Tools & model" section of the Settings dialog: which of cedar's tool groups new
//     conversations get, the default model, turn limit and system prompt (GET/PUT /api/settings).
//   - The "Model server & API keys" section, for admins only (CHAT_ADMIN_TOKEN): the models cedar
//     has registered (each with its server, API key and reasoning levels), its Brave Search key
//     and its Google account. The admin token is kept for this browser session and sent as
//     X-Admin-Token. A change to the models reloads cedarModels (models.js) and dispatches
//     `cedar-models-changed` on `document`, so the apps refill their pickers.
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
  const ALL = '*';

  let deps = null;
  let catalog = null;       // /api/tools, once loaded
  let prefs = null;         // /api/settings
  let pending = undefined;  // tools chosen in a new chat before it exists (undefined: none)
  let chip = null;
  let menu = null;
  let prefsSection = null;
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

  function heading(text, ...extra) {
    const h = el('div', 'cedar-set-heading');
    h.append(el('span', '', text), ...extra);
    return h;
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
  function confirmWrites(names) {
    if (!names.length) return true;
    return confirm(`These tools can change things, not just read them:\n\n${names.join('\n')}\n\nLet the agent use them?`);
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
      box.addEventListener('change', () => {
        if (box.checked && !confirmWrites(writesIn(g))) { box.checked = false; return; }
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
          b.addEventListener('change', () => {
            if (b.checked && t.writes && !confirmWrites([t.name])) { b.checked = false; return; }
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

  // -- Settings: tools & model -------------------------------------------------------------------

  async function renderPrefs() {
    if (!prefsSection) return;
    prefsSection.replaceChildren(heading('Tools & model'), note('Loading…'));
    try {
      [catalog, prefs] = await Promise.all([loadCatalog(true), deps.api('GET', '/settings')]);
    } catch (e) {
      prefsSection.replaceChildren(heading('Tools & model'), note(`Could not load the settings: ${e.message}`, 'cedar-set-error'));
      return;
    }
    // cedar's own defaults (null if cedar couldn't be asked), to show what an unset field means.
    const defaults = prefs.defaults || null;
    const draft = { ...prefs, tools: prefs.tools };
    let state = decode(prefs.tools);
    const save = button('Save defaults', submit, 'primary');
    save.disabled = true;
    const status = el('span', 'cedar-set-status');
    const dirty = () => { save.disabled = false; status.textContent = ''; };

    const picker = toolPicker(state, (s) => { state = s; draft.tools = encode(s); dirty(); }, { detailed: true });

    const fields = el('div', 'cedar-set-fields');
    let modelSelect = null;
    if (cedarModels.all().length) {
      modelSelect = el('select');
      cedarModels.fillSelect(modelSelect, { blank: "cedar's default", selected: prefs.model || '' });
      modelSelect.hidden = false;
      modelSelect.addEventListener('change', dirty);
      const modelField = field('Default model', modelSelect);
      modelField.append(el('small', 'cedar-set-field-note', 'The picker by the message box changes it for one conversation, with the reasoning level next to it.'));
      fields.append(modelField);
    } else if (cedarModels.loaded()) {
      fields.append(note('No model is registered yet: an admin adds one under Model server & API keys below.', 'cedar-set-error'));
    }
    const turns = el('input');
    turns.type = 'number';
    turns.min = '1';
    turns.max = '1000';
    turns.placeholder = defaults ? `Server limit (${defaults.max_turns_cap})` : 'Server limit';
    turns.value = prefs.max_turns ?? '';
    turns.addEventListener('input', dirty);
    fields.append(field('Max turns per message', turns));
    // Without a saved prompt the field shows cedar's default text; it is only saved when changed.
    const defaultSystem = defaults?.system ?? '';
    const system = el('textarea');
    system.rows = 6;
    system.placeholder = defaults ? '' : "cedar's default system prompt (cedar could not be asked for its text)";
    system.value = prefs.system ?? defaultSystem;
    const isDefault = () => system.value.trim() === defaultSystem.trim();
    const tag = el('span', 'cedar-set-badge', "cedar's default");
    const reset = button('Use the default', () => { system.value = defaultSystem; syncSystem(); dirty(); });
    const syncSystem = () => {
      tag.hidden = !isDefault();
      reset.hidden = isDefault();
    };
    system.addEventListener('input', () => { syncSystem(); dirty(); });
    syncSystem();
    const extra = el('span', 'cedar-set-field-extra');
    extra.append(tag, reset);
    const systemField = field('System prompt', system, extra);
    systemField.append(el('small', 'cedar-set-field-note', 'cedar adds a short note after it saying which tools this conversation has and what the sandbox can reach.'));
    fields.append(systemField);

    async function submit() {
      save.disabled = true;
      status.textContent = 'Saving…';
      const body = {
        tools: draft.tools,
        model: modelSelect ? modelSelect.value || null : prefs.model,
        // The default text is saved as "unset", so the prompt keeps following cedar's SYSTEM_PROMPT.
        system: isDefault() || !system.value.trim() ? null : system.value.trim(),
        max_turns: turns.value ? Number(turns.value) : null,
      };
      try {
        prefs = { ...(await deps.api('PUT', '/settings', body)), defaults };
        if (prefs.system === null) { system.value = defaultSystem; syncSystem(); }
        status.textContent = 'Saved';
        updateChip();
      } catch (e) {
        status.textContent = '';
        save.disabled = false;
        deps.toast(e.message);
      }
    }

    const actions = el('div', 'cedar-set-actions');
    actions.append(status, save);
    prefsSection.replaceChildren(
      heading('Tools & model'),
      note('Defaults for every conversation. The Tools button by the message box changes them for one conversation.'),
      picker,
      fields,
      actions,
    );
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

  async function renderIntegrations() {
    if (!integrationsSection) return;
    const cfg = deps.config();
    integrationsSection.hidden = false;
    if (!cfg.admin_enabled) {
      try { await loadCatalog(); } catch { /* the help is shown without the list */ }
      integrationsSection.replaceChildren(heading('Model server & API keys'), setupHelp());
      return;
    }
    const lock = button('Lock', () => { setAdminToken(''); renderIntegrations(); });
    if (!(adminToken() || memoryAdmin)) {
      integrationsSection.replaceChildren(heading('Model server & API keys'), unlockForm());
      return;
    }
    integrationsSection.replaceChildren(heading('Model server & API keys', lock), note('Loading…'));
    let status;
    try {
      status = await adminApi('GET', '/integrations');
    } catch (e) {
      if (e.status === 403) {
        setAdminToken('');
        integrationsSection.replaceChildren(heading('Model server & API keys'), unlockForm(e.message));
      } else {
        integrationsSection.replaceChildren(heading('Model server & API keys', lock), note(e.message, 'cedar-set-error'));
      }
      return;
    }
    const parts = [heading('Model server & API keys', lock)];
    if (!status.storage) {
      parts.push(note('cedar has no CEDAR_SECRET_KEY, so API keys can\'t be saved here: models whose server needs none can still be registered; the Brave key and the Google account can only be set in cedar\'s environment.'));
    }
    parts.push(await modelsBlock(status), braveRow(status), googleRow(status));
    integrationsSection.replaceChildren(...parts);
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
    const wrap = el('div');
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
    const head = el('div', 'cedar-set-subheading');
    head.append(el('strong', '', 'Models'));
    wrap.append(head);
    let entries;
    try {
      ({ models: entries } = await adminApi('GET', '/integrations/models'));
    } catch (e) {
      wrap.append(note(`Could not list the models: ${e.message}`, 'cedar-set-error'));
      return wrap;
    }
    entries = entries || [];
    // The button sits in the heading, so it is in view however long the list is. The form it
    // opens goes right under the heading, above the first entry.
    const intro = entries.length
      ? note('Each model is served by its own OpenAI-compatible server. New chats use the default; the picker by the message box chooses another, and the reasoning level next to it. Add a model for another server, or another model on the same one.')
      : note('No model is registered yet, so messages can\'t be sent. Add the server that runs your model: for a local vLLM, http://localhost:8000/v1 and the name it registered (curl localhost:8000/v1/models).', 'cedar-set-error');
    wrap.append(intro);
    const add = button('Add a model', () => {
      add.hidden = true;
      const form = modelForm(null, status, () => { add.hidden = false; form.remove(); });
      intro.after(form);
      form.querySelector('input').focus();
    }, entries.length ? 'copy-btn' : 'primary');
    add.setAttribute('aria-label', 'Add a model');
    head.append(add);
    for (const entry of entries) wrap.append(modelRow(entry, status));
    return wrap;
  }

  function modelRow(entry, status) {
    const levels = entry.reasoning_levels || [];
    const { row, main, side } = integrationRow(entry.label || entry.id, `${entry.model} at ${entry.base_url}`);
    main.append(el('small', '', `id: ${entry.id}`));
    if (entry.is_default) side.append(badge('Default', 'ok'));
    if (entry.api_key_set) side.append(badge('API key set'));
    if (entry.multimodal) side.append(badge('Images & PDFs'));
    side.append(badge(levels.length ? `Reasoning: ${levels.join(', ')}${entry.default_reasoning ? ` (default ${entry.default_reasoning})` : ''}` : 'No reasoning levels'));
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
      if (!confirm(`Remove the model ${entry.label || entry.id}? Conversations using it fail at their next message until another model is picked.`)) return;
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
    const model = input('Model name on the server, like deepseek-ai/DeepSeek-V4-Flash', entry?.model || '', 'Model name');
    let listed = null;
    if (entry?.listed_models?.length) {
      listed = el('datalist');
      listed.id = `cedar-set-listed-${entry.id}`;
      for (const m of entry.listed_models) listed.append(new Option(m, m));
      model.setAttribute('list', listed.id);
    }
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
    const multimodalLabel = el('label', 'cedar-set-check plain');
    const multimodalBox = el('input');
    multimodalBox.type = 'checkbox';
    multimodalBox.checked = Boolean(entry?.multimodal);
    const multimodalText = el('span');
    multimodalText.append(el('strong', '', 'Accepts images and PDFs'));
    multimodalText.append(el('small', '', 'Shows an attach button by the message box while this model is picked (files can also be dropped or pasted). cedar passes the files to the model as they are; a model that cannot read them fails the message.'));
    multimodalLabel.append(multimodalBox, multimodalText);
    const makeDefault = el('label', 'cedar-set-check small plain');
    const defaultBox = el('input');
    defaultBox.type = 'checkbox';
    defaultBox.checked = Boolean(entry?.is_default);
    defaultBox.disabled = Boolean(entry?.is_default);
    makeDefault.append(defaultBox, el('span', '', 'Use as the default model for new chats'));
    const saveBtn = button(entry ? 'Check and save' : 'Check and add', () => form.requestSubmit(), 'primary');
    form.append(id, label, address, model);
    if (listed) form.append(listed);
    form.append(key);
    if (entry?.api_key_set && canSaveKey) form.append(clearKey);
    form.append(levels, defaultLevel, multimodalLabel, makeDefault);
    form.append(el('small', 'cedar-set-field-note', 'cedar checks the address by listing the server\'s models before saving. DeepSeek V4 on vLLM takes the levels none, low, high, xhigh and max as reasoning_effort; OpenAI-style servers take none, minimal, low, medium and high.'));
    form.append(saveBtn, button('Cancel', onDone));
    form.addEventListener('submit', async (e) => {
      e.preventDefault();
      if (!id.value.trim()) { deps.toast('Give the model an id'); id.focus(); return; }
      if (!address.value.trim()) { deps.toast('Enter the server address'); address.focus(); return; }
      if (!model.value.trim()) { deps.toast('Enter the model name'); model.focus(); return; }
      const body = {
        base_url: address.value.trim(),
        model: model.value.trim(),
        label: label.value.trim() || model.value.trim(),
        reasoning_levels: parseLevels(),
        default_reasoning: defaultLevel.value,
        multimodal: multimodalBox.checked,
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
        if (!confirm('Remove the Brave Search key? Web search stops working for new messages.')) return;
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
        if (!confirm('Disconnect the Google account? Its tools stop working for new messages.')) return;
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
    if (prefsSection && document.getElementById('settings-dialog')?.open) renderPrefs();
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
    document.getElementById('settings-btn')?.click();
  }

  function bind() {
    prefsSection = document.getElementById('prefs-section');
    integrationsSection = document.getElementById('integrations-section');
    const open = document.getElementById('settings-btn');
    // ui-pref.js opens the dialog on this click; the sections are (re)loaded each time it opens.
    open?.addEventListener('click', () => { renderPrefs(); renderIntegrations(); });
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

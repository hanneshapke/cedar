'use strict';

// Settings that change what the agent can do, shared by both UIs (/v1 and /v2). Three pieces:
//
//   - The "Tools & model" section of the Settings dialog: which of cedar's tool groups new
//     conversations get, and the default model, turn limit and system prompt (GET/PUT /api/settings).
//   - The "Integrations" section, for admins only (CHAT_ADMIN_TOKEN): cedar's Brave Search key and
//     its Google account. The admin token is kept for this browser session and sent as X-Admin-Token.
//   - A "Tools" button next to the model picker that changes the tools of one conversation
//     (PUT /api/conversations/{id}/tools). In a new chat the choice is applied when it is created.
//
// Each app hands over its helpers through cedarSettings.init():
//
//   api(method, path, body) -> Promise<json>   the app's /api client (sends the access token)
//   authHeaders() -> object                    the access token header, for the admin calls
//   toast(message)                             a short notice
//   models() -> string[]                       the models offered in the picker (may be empty)
//   config() -> object                         /api/config (admin_enabled)
//   current() -> conversation | null           the open conversation
//   anchor                                     the element the Tools button goes after (the model picker)

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
    const models = deps.models();
    let modelSelect = null;
    if (models.length) {
      modelSelect = el('select');
      modelSelect.append(new Option(defaults?.model ? `cedar's default (${defaults.model})` : "cedar's default", ''));
      for (const m of models) modelSelect.append(new Option(m, m));
      modelSelect.value = prefs.model || '';
      modelSelect.addEventListener('change', dirty);
      fields.append(field('Default model', modelSelect));
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
      parts.push(note('cedar has no CEDAR_SECRET_KEY, so keys can only be set in its environment.'));
    }
    parts.push(llmRow(status), braveRow(status), googleRow(status));
    integrationsSection.replaceChildren(...parts);
  }

  /** Where keys go when this page can't manage them (no CHAT_ADMIN_TOKEN). */
  function setupHelp() {
    const wrap = el('div', 'cedar-set-help');
    wrap.append(note('The model server address, the Brave Search key and the Google account are set on the cedar server. They can be managed here once these are set:'));
    const list = el('ul');
    const item = (code, text) => {
      const li = el('li');
      li.append(el('code', '', code), ` ${text}`);
      list.append(li);
    };
    item('CHAT_ADMIN_TOKEN', 'on cedar-chat: the token that unlocks this section');
    item('CEDAR_SECRET_KEY', 'on cedar (openssl rand -hex 32): encrypts what is saved here');
    item('CEDAR_ADMIN_KEYS', "on cedar: include cedar-chat's CEDAR_API_KEY");
    wrap.append(list);
    wrap.append(note('Until then, set LLM_BASE_URL, LLM_API_KEY, BRAVE_API_KEY and the GOOGLE_* variables in cedar\'s environment (see .env.example). Everything else, like the sandbox and limits, is only set there.'));
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
    wrap.append(note('The model server, API keys and the Google account are for admins. Enter the admin token (CHAT_ADMIN_TOKEN) to manage them.'));
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

  /** The address (and key) of the OpenAI-compatible server cedar sends model calls to. */
  function llmRow(status) {
    const l = status.llm || {};
    const { row, main, side } = integrationRow('Model server', 'The OpenAI-compatible server (vLLM) cedar sends every model call to.');
    side.append(badge(l.base_url || 'unknown', l.source === 'ui' ? 'ok' : ''));
    if (l.source === 'env') side.append(badge('Managed by environment'));
    if (l.source === 'default') side.append(badge('Default'));
    if (l.api_key_set) side.append(badge('API key set'));
    if (Array.isArray(l.models)) {
      const found = l.models.length ? l.models.join(', ') : 'none';
      main.append(el('small', '', `Models it listed: ${found}`));
      if (l.default_model && !l.models.includes(l.default_model)) {
        main.append(el('small', 'cedar-set-warn-text', `cedar's default model ${l.default_model} is not among them; set cedar's LLM_MODEL (and cedar-chat's CHAT_MODELS) to one of these.`));
      }
    }
    if (l.source === 'env' || !status.storage || l.changeable === false) return row;

    let current;
    try { current = new URL(l.base_url); } catch { current = new URL('http://localhost:8000/v1'); }
    const edit = el('form', 'cedar-set-inline cedar-set-endpoint');
    edit.hidden = true;
    const input = (placeholder, value, label, type = 'text') => {
      const i = el('input');
      i.type = type;
      i.placeholder = placeholder;
      i.value = value;
      i.setAttribute('aria-label', label);
      return i;
    };
    const scheme = el('select');
    scheme.setAttribute('aria-label', 'Protocol');
    for (const p of ['http', 'https']) scheme.append(new Option(`${p}://`, p));
    scheme.value = current.protocol.replace(':', '');
    const host = input('Address, e.g. 10.0.0.5', current.hostname, 'Address');
    const port = input('Port', current.port || (scheme.value === 'https' ? '443' : '80'), 'Port', 'number');
    port.min = '1';
    port.max = '65535';
    const path = input('/v1', current.pathname === '/' ? '/v1' : current.pathname, 'Path');
    const key = input(l.api_key_set ? 'API key (leave empty to keep it)' : 'API key, if the server needs one', '', 'API key', 'password');
    key.autocomplete = 'off';
    const clearKey = el('label', 'cedar-set-check small');
    const clearBox = el('input');
    clearBox.type = 'checkbox';
    clearKey.append(clearBox, el('span', '', 'Remove the saved API key'));
    const saveBtn = button('Check and save', () => edit.requestSubmit(), 'primary');
    const addr = el('div', 'cedar-set-endpoint-row');
    addr.append(scheme, host, el('span', 'cedar-set-colon', ':'), port, path);
    edit.append(addr, key);
    if (l.api_key_source === 'ui') edit.append(clearKey);
    edit.append(saveBtn, button('Cancel', () => { edit.hidden = true; }));
    edit.addEventListener('submit', async (e) => {
      e.preventDefault();
      if (!host.value.trim() || !port.value) { deps.toast('Enter the address and port'); return; }
      const p = path.value.trim() || '/v1';
      const body = { base_url: `${scheme.value}://${host.value.trim()}:${port.value}${p.startsWith('/') ? p : `/${p}`}` };
      if (key.value.trim()) body.api_key = key.value.trim();
      else if (clearBox.checked) body.api_key = '';
      saveBtn.disabled = true;
      saveBtn.textContent = 'Checking…';
      try {
        const saved = await adminApi('PUT', '/integrations/llm', body);
        deps.toast(`Model server set to ${saved.base_url}`);
        renderIntegrations();
      } catch (err) {
        saveBtn.disabled = false;
        saveBtn.textContent = 'Check and save';
        deps.toast(err.message);
      }
    });
    side.append(button('Change', () => { edit.hidden = false; host.focus(); }));
    if (l.source === 'ui') {
      side.append(button('Use the default', async () => {
        if (!confirm('Forget this address (and its saved API key) and go back to the server default?')) return;
        try {
          await adminApi('DELETE', '/integrations/llm');
          renderIntegrations();
        } catch (err) { deps.toast(err.message); }
      }));
    }
    row.append(edit); // full width, below the description and the buttons
    return row;
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

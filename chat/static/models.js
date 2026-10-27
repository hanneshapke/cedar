'use strict';

// The models cedar has registered (GET /api/models), shared by both UIs (/v1 and /v2) and by
// settings.js and schedule.js. Each entry is cedar's
// `{id, label, model, reasoning_levels, default_reasoning, is_default}`. This file fills the
// pickers: a model <select> (ids, shown by label) and a reasoning <select> that offers the chosen
// model's levels and hides itself when the model has none.
//
//   load(api) -> Promise<entries>      fetch the registry with the app's /api client
//   all() -> entries                   as last loaded
//   loaded() -> boolean                whether a load has succeeded (an empty registry counts)
//   byId(id) -> entry | null
//   defaultId() -> string              the default entry's id ('' when nothing is registered)
//   levelsFor(id) -> string[]          the levels of that model ('' or unknown: the default model)
//   multimodal(id) -> boolean          whether that model ('' or unknown: the default one) takes
//                                      images
//   fillSelect(select, {blank, selected}) -> boolean   the model picker; false when there is
//                                      nothing to pick (fewer than two models and no `blank`)
//   fillLevels(select, modelId, selected) -> boolean   the reasoning picker; false when hidden
//   bindPair(modelSelect, levelSelect) refill the levels when the model changes
//   sendMenu({send, modelSelect, levelSelect}) -> {refresh}
//                                      the send button's menu: a label next to the button names
//                                      the chosen model and level and opens a menu of the models
//                                      with their levels; choosing writes the two selects (and
//                                      fires their change events). The selects stay hidden
//                                      (models.css) but remain the state the app reads and
//                                      restores. Call refresh() after filling the selects.
//
// After an admin changes the registry, settings.js reloads it and dispatches
// `cedar-models-changed` on `document`; the apps refill their pickers on it.

const cedarModels = (() => {
  let list = [];
  let ready = false;

  async function load(api) {
    const body = await api('GET', '/models');
    list = Array.isArray(body?.models) ? body.models : [];
    ready = true;
    return list;
  }

  const all = () => list;
  const loaded = () => ready;
  const byId = (id) => list.find((m) => m.id === id) || null;
  const defaultEntry = () => list.find((m) => m.is_default) || list[0] || null;
  const defaultId = () => defaultEntry()?.id || '';
  const resolve = (id) => (id && byId(id)) || defaultEntry();
  const levelsFor = (id) => resolve(id)?.reasoning_levels || [];
  const multimodal = (id) => Boolean(resolve(id)?.multimodal);

  /** A reasoning level as shown: the common ones in the page's language, others as registered. */
  function levelName(level) {
    const key = `models.level.${level}`;
    return cedarI18n.has(key) ? tr(key) : level;
  }

  /** The model's label, with its default level in parentheses when it has one. */
  function label(entry) {
    return entry.label || entry.model || entry.id;
  }

  /**
   * Fill a model <select>. `blank` is the text of a first option whose value is '' (meaning
   * cedar's default, named in it); `null` leaves it out, and the default entry is then selected
   * when `selected` is unknown. The select is hidden when there is nothing to choose between.
   */
  function fillSelect(select, { blank = tr('models.defaultModel'), selected = '' } = {}) {
    select.textContent = '';
    const def = defaultEntry();
    const withBlank = blank !== null && blank !== undefined;
    if (withBlank) select.append(new Option(def ? tr('models.withDefault', { blank, model: label(def) }) : blank, ''));
    for (const m of list) select.append(new Option(label(m), m.id));
    select.hidden = list.length < 2;
    select.value = byId(selected) ? selected : withBlank ? '' : defaultId();
    return !select.hidden;
  }

  /**
   * Fill a reasoning <select> with the levels of `modelId` ('' for the default model): a first
   * option for the model's own default, then each level. Hidden when the model offers none.
   */
  function fillLevels(select, modelId, selected = '') {
    const entry = resolve(modelId);
    const levels = entry?.reasoning_levels || [];
    select.textContent = '';
    select.hidden = !levels.length;
    if (!levels.length) return false;
    const def = entry.default_reasoning;
    select.append(new Option(def ? tr('models.reasoningDefaultIs', { level: levelName(def) }) : tr('models.reasoningDefault'), ''));
    for (const l of levels) select.append(new Option(tr('models.reasoning', { level: levelName(l) }), l));
    select.value = levels.includes(selected) ? selected : '';
    return true;
  }

  function bindPair(modelSelect, levelSelect) {
    modelSelect.addEventListener('change', () => fillLevels(levelSelect, modelSelect.value, ''));
  }

  const CHEVRON = '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M6 15l6-6 6 6"/></svg>';
  const TICK = '<svg class="tick" viewBox="0 0 24 24" aria-hidden="true" style="fill:none;stroke:currentColor;stroke-width:2.5"><path d="M5 12l5 5 9-10"/></svg>';

  function sendMenu({ send, modelSelect, levelSelect }) {
    const group = document.createElement('span');
    group.className = 'cedar-send-group';
    send.before(group);
    const pick = document.createElement('button');
    pick.type = 'button';
    pick.className = 'cedar-send-pick';
    pick.setAttribute('aria-haspopup', 'menu');
    pick.setAttribute('aria-expanded', 'false');
    pick.title = tr('models.pickTitle');
    const labelEl = document.createElement('span');
    labelEl.className = 'cedar-send-pick-label';
    pick.append(labelEl);
    pick.insertAdjacentHTML('beforeend', CHEVRON);
    const menu = document.createElement('div');
    menu.className = 'cedar-send-menu';
    menu.setAttribute('role', 'menu');
    menu.setAttribute('aria-label', tr('models.menuLabel'));
    menu.hidden = true;
    group.append(pick, send, menu);

    const chosenModel = () => resolve(modelSelect.value);
    const levelOf = () => levelSelect.value || '';

    function refresh() {
      const entry = chosenModel();
      const choosable = list.length >= 2 || (entry?.reasoning_levels || []).length > 0;
      pick.hidden = !entry || !choosable;
      if (!entry) { close(); return; }
      labelEl.textContent = levelOf() ? `${label(entry)} · ${levelName(levelOf())}` : label(entry);
      if (!menu.hidden) render();
    }

    /** Set the selects and let the app's listeners (levels, paperclip) follow. */
    function choose(id, level) {
      modelSelect.value = id;
      modelSelect.dispatchEvent(new Event('change'));
      if (level && [...levelSelect.options].some((o) => o.value === level)) levelSelect.value = level;
      else levelSelect.value = '';
      levelSelect.dispatchEvent(new Event('change'));
      close();
      refresh();
    }

    function render() {
      menu.textContent = '';
      const title = document.createElement('div');
      title.className = 'cedar-send-menu-title';
      title.textContent = tr('models.sendWith');
      menu.append(title);
      const current = chosenModel();
      for (const m of list) {
        const item = document.createElement('div');
        item.className = 'cedar-send-item';
        const isCurrent = current?.id === m.id;
        if (isCurrent) item.classList.add('current');
        const b = document.createElement('button');
        b.type = 'button';
        b.className = 'cedar-send-model';
        b.setAttribute('role', 'menuitemradio');
        b.setAttribute('aria-checked', String(isCurrent));
        b.insertAdjacentHTML('beforeend', TICK);
        const name = document.createElement('span');
        name.className = 'name';
        name.textContent = label(m);
        b.append(name);
        if (m.is_default) {
          const tag = document.createElement('span');
          tag.className = 'tag';
          tag.textContent = tr('models.defaultTag');
          b.append(tag);
        }
        b.addEventListener('click', () => choose(m.id, isCurrent ? levelOf() : (m.default_reasoning ? '' : '')));
        item.append(b);
        const levels = m.reasoning_levels || [];
        if (levels.length) {
          const row = document.createElement('div');
          row.className = 'cedar-send-levels';
          const chip = (value, text, checked) => {
            const c = document.createElement('button');
            c.type = 'button';
            c.className = 'cedar-send-level';
            c.setAttribute('role', 'menuitemradio');
            c.setAttribute('aria-checked', String(checked));
            c.textContent = text;
            c.addEventListener('click', () => choose(m.id, value));
            return c;
          };
          row.append(chip('', m.default_reasoning ? tr('models.defaultLevelIs', { level: levelName(m.default_reasoning) }) : tr('models.defaultTag'), isCurrent && !levelOf()));
          for (const l of levels) row.append(chip(l, levelName(l), isCurrent && levelOf() === l));
          item.append(row);
        }
        menu.append(item);
      }
    }

    function open() {
      render();
      menu.hidden = false;
      pick.setAttribute('aria-expanded', 'true');
      menu.querySelector('[aria-checked="true"]')?.focus();
    }
    function close() {
      if (menu.hidden) return;
      menu.hidden = true;
      pick.setAttribute('aria-expanded', 'false');
    }
    pick.addEventListener('click', () => (menu.hidden ? open() : close()));
    document.addEventListener('click', (e) => { if (!group.contains(e.target)) close(); });
    document.addEventListener('keydown', (e) => { if (e.key === 'Escape' && !menu.hidden) { close(); pick.focus(); } });
    modelSelect.addEventListener('change', refresh);
    levelSelect.addEventListener('change', refresh);
    refresh();
    return { refresh };
  }

  return { load, all, loaded, byId, defaultId, levelsFor, multimodal, levelName, fillSelect, fillLevels, bindPair, sendMenu };
})();

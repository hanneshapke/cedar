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
//                                      images and PDFs
//   fillSelect(select, {blank, selected}) -> boolean   the model picker; false when there is
//                                      nothing to pick (fewer than two models and no `blank`)
//   fillLevels(select, modelId, selected) -> boolean   the reasoning picker; false when hidden
//   bindPair(modelSelect, levelSelect) refill the levels when the model changes
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

  /** The model's label, with its default level in parentheses when it has one. */
  function label(entry) {
    return entry.label || entry.model || entry.id;
  }

  /**
   * Fill a model <select>. `blank` is the text of a first option whose value is '' (meaning
   * cedar's default, named in it); `null` leaves it out, and the default entry is then selected
   * when `selected` is unknown. The select is hidden when there is nothing to choose between.
   */
  function fillSelect(select, { blank = 'Default model', selected = '' } = {}) {
    select.textContent = '';
    const def = defaultEntry();
    const withBlank = blank !== null && blank !== undefined;
    if (withBlank) select.append(new Option(def ? `${blank} (${label(def)})` : blank, ''));
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
    select.append(new Option(def ? `Reasoning: default (${def})` : 'Reasoning: default', ''));
    for (const l of levels) select.append(new Option(`Reasoning: ${l}`, l));
    select.value = levels.includes(selected) ? selected : '';
    return true;
  }

  function bindPair(modelSelect, levelSelect) {
    modelSelect.addEventListener('change', () => fillLevels(levelSelect, modelSelect.value, ''));
  }

  return { load, all, loaded, byId, defaultId, levelsFor, multimodal, fillSelect, fillLevels, bindPair };
})();

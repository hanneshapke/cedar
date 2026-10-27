'use strict';

// Scheduled agents, shared by every UI. Three pieces:
//
//   - "Schedule this agent": a dialog that has cedar compact the open conversation into one
//     prompt, lets the user edit it and pick a local time and frequency, and creates the schedule
//     (or edits an existing one).
//   - The "Scheduled agents" list in the conversations sidebar: every schedule with its next run,
//     drawn into each `[data-schedules]` mount the page has (GET /api/schedules).
//   - A schedule's detail, in the same dialog: its timing, prompt, pause/resume, run now, edit and
//     delete, and a run history with each run's result.
//
// Each app hands over its own helpers through cedarSchedule.init(), so this file knows nothing
// about the app's internals, and calls cedarSchedule.start() once the access token is known:
//
//   api(method, path, body) -> Promise<json>   the app's /api client (sends the access token)
//   toast(message)                             a short notice
//   renderMarkdown(s) -> html string           the app's Markdown renderer, which escapes all text
//
// The model and reasoning pickers take cedar's registry from cedarModels (models.js).

const cedarSchedule = (() => {
  // Each frequency is named by schedule.freq.<id> in the catalog.
  const FREQUENCIES = ['daily', 'weekdays', 'weekly'];
  /** Monday to Sunday in the page's language (1 January 2024 was a Monday). */
  const DAYS = Array.from({ length: 7 }, (_, i) => new Date(2024, 0, 1 + i).toLocaleDateString(cedarI18n.locale, { weekday: 'long' }));
  /** A run's or turn's state as shown; one the catalog doesn't know as it is. */
  const stateName = (state) => (cedarI18n.has(`schedule.state.${state}`) ? tr(`schedule.state.${state}`) : state);
  const ACTIVE = new Set(['queued', 'running']);
  const PREVIEW_CHARS = 600;
  const REFRESH_MS = 10_000;

  let deps = null;
  let form = null;      // the schedule dialog and its parts, built on first use
  let editing = null;   // { conversation } creating, { schedule, returnTo } editing, { detail: id } viewing
  let schedules = [];   // GET /schedules, as last loaded
  let started = false;
  let refreshTimer = null;
  let request = 0;      // bumped on every (re)render so a slow fetch can't draw into a newer view

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

  function localZone() {
    try { return Intl.DateTimeFormat().resolvedOptions().timeZone || 'UTC'; } catch { return 'UTC'; }
  }

  /** "Mon 12 Oct, 09:00" in the viewer's locale. */
  function when(iso) {
    if (!iso) return '';
    return new Date(iso).toLocaleString(cedarI18n.locale, { weekday: 'short', day: 'numeric', month: 'short', hour: '2-digit', minute: '2-digit' });
  }

  /** "Notifies by email", for the list. */
  function notifyText(s) {
    if (!s.notify) return '';
    if (s.notify === 'none') return tr('schedule.onlyInApp');
    if (typeof cedarNotify === 'undefined') return tr('schedule.notifiesBy', { method: s.notify });
    // German nouns keep their capital letter.
    const label = cedarNotify.methodLabel(s.notify);
    return tr('schedule.notifiesBy', { method: cedarI18n.lang === 'de' ? label : label.toLowerCase() });
  }

  /** "Every weekday at 09:00 (Europe/Berlin)". */
  function describe(s) {
    const time = s.time_of_day;
    let text;
    if (s.frequency === 'weekly' && s.weekday != null) text = tr('schedule.at.weeklyOn', { day: DAYS[s.weekday], time });
    else if (FREQUENCIES.includes(s.frequency)) text = tr(`schedule.at.${s.frequency}`, { time });
    else text = tr('schedule.at.other', { frequency: s.frequency, time });
    const zone = s.timezone === localZone() ? '' : ` (${s.timezone})`;
    return `${text}${zone}`;
  }

  /** The next full hour, as HH:MM. */
  function defaultTime() {
    const d = new Date();
    d.setHours(d.getHours() + 1, 0, 0, 0);
    return `${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}`;
  }

  /** What a run is up to: a label and a class (ok, err, live, muted). */
  function runState(run) {
    if (run.error) return [tr('schedule.state.notStarted'), 'err'];
    const t = run.turn;
    if (!t) return [run.conversation_id ? tr('schedule.state.noResponse') : tr('schedule.state.deleted'), 'muted'];
    if (ACTIVE.has(t.status)) return [stateName(t.status), 'live'];
    if (t.status === 'completed') return [stateName('completed'), 'ok'];
    return [stateName(t.status), 'err'];
  }

  function chip(run) {
    const [label, cls] = runState(run);
    const c = el('span', `cedar-sched-chip ${cls}`, label);
    if (cls === 'live') c.prepend(el('span', 'spinner'));
    return c;
  }

  function openConversation(id) {
    if (form?.dialog.open) form.dialog.close();
    location.hash = `#/c/${id}`;
  }

  // -- the schedule dialog -----------------------------------------------------------------------

  function field(labelText, control) {
    const wrap = el('label', 'cedar-sched-field');
    wrap.append(el('span', '', labelText), control);
    return wrap;
  }

  function buildForm() {
    const dialog = el('dialog', 'cedar-sched-dialog');
    dialog.setAttribute('aria-labelledby', 'cedar-sched-title');
    const head = el('div', 'cedar-sched-head');
    const title = el('h3', '', tr('schedule.create'));
    title.id = 'cedar-sched-title';
    head.append(title, button(tr('schedule.close'), () => dialog.close()));

    // Stage 1: the conversation is being compacted.
    const review = el('div', 'cedar-sched-review');
    const reviewText = el('span', '', tr('schedule.reviewing'));
    const retry = button(tr('schedule.tryAgain'), () => compactNow());
    retry.hidden = true;
    review.append(el('span', 'spinner'), reviewText, retry);

    // Stage 2: the form.
    const f = el('form', 'cedar-sched-form');
    const titleInput = el('input');
    titleInput.maxLength = 200;
    titleInput.required = true;
    titleInput.placeholder = tr('schedule.namePlaceholder');
    const prompt = el('textarea');
    prompt.rows = 9;
    prompt.required = true;
    prompt.placeholder = tr('schedule.promptPlaceholder');
    const promptHelp = el('div', 'cedar-sched-help');
    const regenerate = button(tr('schedule.regenerate'), () => compactNow());
    promptHelp.append(el('span', '', tr('schedule.promptHelp')), regenerate);
    const frequency = el('select');
    for (const value of FREQUENCIES) frequency.append(new Option(tr(`schedule.freq.${value}`), value));
    const weekday = el('select');
    DAYS.forEach((d, i) => weekday.append(new Option(d, String(i))));
    const weekdayField = field(tr('schedule.weekday'), weekday);
    const time = el('input');
    time.type = 'time';
    time.required = true;
    const zone = el('div', 'cedar-sched-zone');
    const model = el('select');
    const modelField = field(tr('schedule.model'), model);
    const reasoning = el('select');
    const reasoningField = field(tr('schedule.reasoning'), reasoning);
    cedarModels.bindPair(model, reasoning);
    model.addEventListener('change', () => { reasoningField.hidden = reasoning.hidden; });
    // How a finished run is announced: the default from Settings → Notifications, not at all, or
    // one of the methods set up there (cedarNotify knows which).
    const notify = el('select');
    const notifyField = field(tr('schedule.notifyMeBy'), notify);
    const notifyHelp = el('div', 'cedar-sched-help');
    notifyHelp.append(el('span', '', tr('schedule.notifyHelp')));
    const error = el('div', 'cedar-sched-error');
    error.hidden = true;
    const actions = el('div', 'cedar-sched-actions');
    const cancel = button(tr('schedule.cancel'), () => dialog.close());
    const submit = el('button', 'primary', tr('schedule.schedule'));
    submit.type = 'submit';
    actions.append(cancel, submit);

    const row = el('div', 'cedar-sched-row');
    row.append(field(tr('schedule.frequency'), frequency), weekdayField, field(tr('schedule.time'), time));
    f.append(field(tr('schedule.name'), titleInput), field(tr('schedule.prompt'), prompt), promptHelp, row, zone, modelField, reasoningField, notifyField, notifyHelp, error, actions);
    frequency.addEventListener('change', () => { weekdayField.hidden = frequency.value !== 'weekly'; });
    f.addEventListener('submit', (e) => { e.preventDefault(); submitForm(); });

    // Stage 3: a schedule's detail.
    const detail = el('div', 'cedar-sched-detail');

    dialog.append(head, review, f, detail);
    dialog.addEventListener('close', () => { editing = null; stopRefresh(); });
    document.body.append(dialog);
    return { dialog, title, review, reviewText, retry, form: f, titleInput, prompt, promptHelp, frequency, weekday, weekdayField, time, zone, model, modelField, reasoning, reasoningField, notify, notifyField, error, submit, detail };
  }

  function showStage(stage) {
    form.review.hidden = stage !== 'review';
    form.form.hidden = stage !== 'form';
    form.detail.hidden = stage !== 'detail';
  }

  function fillModels(selected, level) {
    const shown = cedarModels.fillSelect(form.model, { selected: selected || '' });
    form.model.hidden = false;
    form.modelField.hidden = !shown;
    form.reasoningField.hidden = !cedarModels.fillLevels(form.reasoning, form.model.value, level || '');
  }

  /** Fill the notify picker with the methods that are set up; `selected` is a schedule's choice (null: the default). */
  async function fillNotify(selected) {
    const current = selected ?? '';
    form.notify.textContent = '';
    form.notify.append(new Option(tr('schedule.default'), ''), new Option(tr('schedule.onlyInApp'), 'none'));
    form.notify.value = current === 'none' ? 'none' : '';
    if (typeof cedarNotify === 'undefined') return;
    try { await cedarNotify.loadStatus(true); } catch { return; }
    form.notify.textContent = '';
    for (const { value, label } of cedarNotify.methodOptions(true)) form.notify.append(new Option(label, value));
    if (current && ![...form.notify.options].some((o) => o.value === current)) {
      form.notify.append(new Option(tr('schedule.noLongerSetUp', { method: cedarNotify.methodLabel(current) }), current));
    }
    form.notify.value = current;
  }

  function setTiming({ frequency = 'daily', weekday = null, time = defaultTime(), timezone = localZone() } = {}) {
    form.frequency.value = frequency;
    form.weekday.value = String(weekday ?? (new Date().getDay() + 6) % 7);
    form.weekdayField.hidden = frequency !== 'weekly';
    form.time.value = time;
    form.zone.textContent = tr('schedule.zone', { zone: timezone });
    form.zone.dataset.zone = timezone;
  }

  /** Start scheduling the open conversation: compact it, then show the form. */
  function openCreate(conversation, { model, reasoning } = {}) {
    if (!deps || !conversation) return;
    form ??= buildForm();
    editing = { conversation };
    form.title.textContent = tr('schedule.create');
    form.submit.textContent = tr('schedule.schedule');
    form.promptHelp.hidden = false;
    form.titleInput.value = conversation.title || '';
    form.prompt.value = '';
    form.error.hidden = true;
    fillModels(model || conversation.model || '', reasoning ?? conversation.reasoning ?? '');
    setTiming();
    fillNotify(null);
    if (!form.dialog.open) form.dialog.showModal();
    compactNow();
  }

  /** Edit an existing schedule; with `returnTo: 'detail'` the dialog goes back to its detail after. */
  function openEdit(schedule, { returnTo } = {}) {
    if (!deps) return;
    form ??= buildForm();
    stopRefresh();
    editing = { schedule, returnTo };
    form.title.textContent = tr('schedule.edit');
    form.submit.textContent = tr('schedule.save');
    form.promptHelp.hidden = true;
    form.titleInput.value = schedule.title;
    form.prompt.value = schedule.prompt;
    form.error.hidden = true;
    fillModels(schedule.model || '', schedule.reasoning || '');
    setTiming({ frequency: schedule.frequency, weekday: schedule.weekday, time: schedule.time_of_day, timezone: schedule.timezone });
    fillNotify(schedule.notify ?? null);
    showStage('form');
    if (!form.dialog.open) form.dialog.showModal();
    form.titleInput.focus();
  }

  async function compactNow() {
    const conversation = editing?.conversation;
    if (!conversation) return;
    showStage('review');
    form.retry.hidden = true;
    form.reviewText.textContent = tr('schedule.reviewing');
    const id = ++request;
    try {
      const { prompt } = await deps.api('POST', `/conversations/${conversation.id}/compact`);
      if (id !== request || editing?.conversation !== conversation) return;
      form.prompt.value = prompt;
      showStage('form');
      form.prompt.focus();
    } catch (e) {
      if (id !== request || editing?.conversation !== conversation) return;
      form.reviewText.textContent = e.message || tr('schedule.reviewFailed');
      form.retry.hidden = false;
    }
  }

  async function submitForm() {
    if (!editing) return;
    const body = {
      title: form.titleInput.value.trim(),
      prompt: form.prompt.value.trim(),
      frequency: form.frequency.value,
      time: form.time.value,
      timezone: form.zone.dataset.zone || localZone(),
    };
    if (body.frequency === 'weekly') body.weekday = Number(form.weekday.value);
    if (!form.modelField.hidden) body.model = form.model.value;
    body.reasoning = form.reasoningField.hidden ? '' : form.reasoning.value;
    body.notify = form.notify.value || null;
    form.error.hidden = true;
    form.submit.disabled = true;
    try {
      let schedule;
      const back = editing.returnTo === 'detail' && editing.schedule;
      if (editing.schedule) {
        schedule = await deps.api('PATCH', `/schedules/${editing.schedule.id}`, body);
        deps.toast(tr('schedule.savedToast', { when: when(schedule.next_run_at) }));
      } else {
        body.conversation_id = editing.conversation.id;
        schedule = await deps.api('POST', '/schedules', body);
        deps.toast(tr('schedule.scheduledToast', { when: when(schedule.next_run_at) }));
      }
      loadSide();
      if (back) openDetail(schedule.id);
      else form.dialog.close();
    } catch (e) {
      form.error.textContent = e.message || tr('schedule.saveFailed');
      form.error.hidden = false;
    } finally {
      form.submit.disabled = false;
    }
  }

  // -- the sidebar list ----------------------------------------------------------------------

  function stopRefresh() {
    clearTimeout(refreshTimer);
    refreshTimer = null;
  }

  /** "Mon 07:00" for a weekly schedule, "07:00" otherwise, for the sidebar. */
  function nextShort(s) {
    if (!s.enabled) return tr('schedule.pausedShort');
    if (!s.next_run_at) return '';
    const d = new Date(s.next_run_at);
    const time = d.toLocaleTimeString(cedarI18n.locale, { hour: '2-digit', minute: '2-digit', hourCycle: 'h23' });
    if (s.frequency === 'daily') return time;
    return `${d.toLocaleDateString(cedarI18n.locale, { weekday: 'short' })} ${time}`;
  }

  const CLOCK = '<svg viewBox="0 0 24 24" aria-hidden="true"><circle cx="12" cy="12" r="9"></circle><path d="M12 7v5l3 2"></path></svg>';

  /** Draw the schedules into every sidebar mount (`[data-schedules]`); hidden when there are none. */
  function renderSide() {
    for (const mount of document.querySelectorAll('[data-schedules]')) {
      mount.hidden = !schedules.length;
      const count = mount.querySelector('.cedar-sched-count');
      if (count) count.textContent = String(schedules.length);
      const list = mount.querySelector('.cedar-sched-links');
      if (!list) continue;
      list.textContent = '';
      for (const s of schedules) {
        const link = el('button', `cedar-sched-link${s.enabled ? '' : ' paused'}`);
        link.type = 'button';
        link.title = [s.title, describe(s)].join(' · ');
        const name = el('span', 'cedar-sched-link-name');
        name.insertAdjacentHTML('beforeend', CLOCK);
        name.append(el('span', 'cedar-sched-link-title', s.title));
        link.append(name, el('span', 'cedar-sched-link-when', nextShort(s)));
        link.addEventListener('click', () => openDetail(s.id));
        list.append(link);
      }
    }
  }

  /** Fetch the schedules and redraw the sidebar. */
  async function loadSide() {
    if (!deps) return;
    try { ({ schedules } = await deps.api('GET', '/schedules')); } catch { return; }
    schedules ||= [];
    renderSide();
  }

  // -- a schedule's detail, in the schedule dialog ----------------------------------------------

  /** Open one schedule: its timing, prompt, actions and runs. */
  async function openDetail(id) {
    if (!deps) return;
    form ??= buildForm();
    editing = { detail: id };
    stopRefresh();
    form.title.textContent = schedules.find((s) => s.id === id)?.title || tr('schedule.agent');
    form.detail.replaceChildren(el('div', 'cedar-sched-review-text', tr('schedule.loading')));
    showStage('detail');
    if (!form.dialog.open) form.dialog.showModal();
    await renderDetail(id);
  }

  async function renderDetail(id) {
    const req = ++request;
    let data;
    try {
      data = await deps.api('GET', `/schedules/${id}`);
    } catch (e) {
      if (req !== request || editing?.detail !== id) return;
      form.detail.replaceChildren(el('div', 'cedar-sched-error', e.message || tr('schedule.loadFailed')));
      return;
    }
    if (req !== request || editing?.detail !== id) return;
    const { schedule: s, runs } = data;
    form.title.textContent = s.title;
    const main = el('div', 'cedar-sched-item-main');
    main.append(el('div', 'cedar-sched-sub', [describe(s), notifyText(s)].filter(Boolean).join(' · ')));
    main.append(el('div', 'cedar-sched-sub', s.enabled ? tr('schedule.nextRun', { when: when(s.next_run_at) }) : tr('schedule.paused')));
    const promptBox = el('details', 'cedar-sched-prompt');
    promptBox.append(el('summary', '', tr('schedule.prompt')), el('pre', '', s.prompt));
    const actions = el('div', 'cedar-sched-item-actions');
    if (s.source_conversation_id) actions.append(button(tr('schedule.open'), () => openConversation(s.source_conversation_id)));
    actions.append(
      button(tr('schedule.runNow'), (e) => runNow(s, e.currentTarget)),
      button(s.enabled ? tr('schedule.pause') : tr('schedule.resume'), () => setEnabled(s, !s.enabled)),
      button(tr('schedule.editButton'), () => openEdit(s, { returnTo: 'detail' })),
      button(tr('schedule.delete'), () => remove(s)),
    );
    const parts = [main, promptBox, actions, el('div', 'cedar-sched-runs-label', tr('schedule.runs', { count: runs.length }))];
    if (!runs.length) parts.push(el('div', 'empty-list', tr('schedule.noRuns')));
    let live = false;
    for (const run of runs) {
      const [, cls] = runState(run);
      live ||= cls === 'live';
      parts.push(runRow(run));
    }
    form.detail.replaceChildren(...parts);
    if (live && form.dialog.open) refreshTimer = setTimeout(() => { if (editing?.detail === id) renderDetail(id); }, REFRESH_MS);
  }

  function runRow(run) {
    const row = el('div', 'cedar-sched-run');
    const head = el('div', 'cedar-sched-run-head');
    head.append(el('span', 'cedar-sched-when', when(run.started_at)), chip(run));
    if (run.manual) head.append(el('span', 'cedar-sched-chip muted', tr('schedule.manualRun')));
    if (run.conversation_id) head.append(button(tr('schedule.open'), () => openConversation(run.conversation_id)));
    row.append(head);
    const body = el('div', 'cedar-sched-run-body');
    if (run.error) {
      body.append(el('div', 'cedar-sched-error', run.error));
    } else if (run.turn?.error) {
      body.append(el('div', 'cedar-sched-error', run.turn.error));
    } else if (run.turn?.final_text) {
      const text = run.turn.final_text;
      const md = el('div', 'md cedar-sched-result');
      const short = text.length > PREVIEW_CHARS;
      md.innerHTML = deps.renderMarkdown(short ? `${text.slice(0, PREVIEW_CHARS)}…` : text);
      body.append(md);
      if (short) {
        const more = button(tr('schedule.showAll'), () => { md.innerHTML = deps.renderMarkdown(text); more.remove(); });
        body.append(more);
      }
    } else if (run.turn && ACTIVE.has(run.turn.status)) {
      body.append(el('div', 'cedar-sched-sub', tr('schedule.working')));
    }
    if (body.childElementCount) row.append(body);
    return row;
  }

  async function runNow(s, btn) {
    if (btn) btn.disabled = true;
    try {
      await deps.api('POST', `/schedules/${s.id}/run`);
      deps.toast(tr('schedule.started', { title: s.title }));
      loadSide();
      if (editing?.detail === s.id) renderDetail(s.id);
    } catch (e) {
      deps.toast(e.message);
      if (btn) btn.disabled = false;
    }
  }

  async function setEnabled(s, enabled) {
    try {
      await deps.api('PATCH', `/schedules/${s.id}`, { enabled });
      loadSide();
      if (editing?.detail === s.id) renderDetail(s.id);
    } catch (e) {
      deps.toast(e.message);
    }
  }

  async function remove(s) {
    if (!(await cedarConfirm(tr('schedule.confirmDelete', { title: s.title }), { ok: tr('schedule.delete') }))) return;
    try {
      await deps.api('DELETE', `/schedules/${s.id}`);
      if (form?.dialog.open) form.dialog.close();
      loadSide();
    } catch (e) {
      deps.toast(e.message);
    }
  }

  function bind() {
    window.addEventListener('hashchange', () => { if (form?.dialog.open) form.dialog.close(); });
    document.addEventListener('visibilitychange', () => { if (document.visibilityState === 'visible' && started) loadSide(); });
  }

  function init(helpers) {
    deps = helpers;
    if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', bind);
    else bind();
  }

  /** Load the sidebar list; once, after the access token is known. */
  function start() {
    if (started || !deps) return;
    started = true;
    loadSide();
  }

  return { init, start, openCreate, openEdit, openDetail, describe };
})();

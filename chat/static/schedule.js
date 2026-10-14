'use strict';

// Scheduled agents, shared by both UIs (/v1 and /v2). Two pieces:
//
//   - "Schedule this agent": a dialog that has cedar compact the open conversation into one
//     prompt, lets the user edit it and pick a local time and frequency, and creates the schedule
//     (or, from Settings, edits an existing one).
//   - The "Scheduled agents" section of the Settings dialog: every schedule with its next and
//     last run, pause/resume, run now, edit and delete, and a run history with each run's result.
//
// Each app hands over its own helpers through cedarSchedule.init(), so this file knows nothing
// about the app's internals:
//
//   api(method, path, body) -> Promise<json>   the app's /api client (sends the access token)
//   toast(message)                             a short notice
//   renderMarkdown(s) -> html string           the app's Markdown renderer, which escapes all text
//
// The model and reasoning pickers take cedar's registry from cedarModels (models.js).

const cedarSchedule = (() => {
  const FREQUENCIES = [['daily', 'Every day'], ['weekdays', 'Weekdays (Mon–Fri)'], ['weekly', 'Every week']];
  const DAYS = ['Monday', 'Tuesday', 'Wednesday', 'Thursday', 'Friday', 'Saturday', 'Sunday'];
  const ACTIVE = new Set(['queued', 'running']);
  const PREVIEW_CHARS = 600;
  const REFRESH_MS = 10_000;

  let deps = null;
  let form = null;      // the schedule dialog and its parts, built on first use
  let editing = null;   // { conversation } when creating, { schedule } when editing
  let section = null;   // the Settings section
  let settings = null;  // the Settings dialog
  let view = null;      // 'list' | { detail: id }
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
    return new Date(iso).toLocaleString([], { weekday: 'short', day: 'numeric', month: 'short', hour: '2-digit', minute: '2-digit' });
  }

  /** "Every weekday at 09:00 (Europe/Berlin)". */
  function describe(s) {
    const freq = FREQUENCIES.find(([k]) => k === s.frequency)?.[1] || s.frequency;
    const day = s.frequency === 'weekly' && s.weekday != null ? ` on ${DAYS[s.weekday]}` : '';
    const zone = s.timezone === localZone() ? '' : ` (${s.timezone})`;
    return `${freq}${day} at ${s.time_of_day}${zone}`;
  }

  /** The next full hour, as HH:MM. */
  function defaultTime() {
    const d = new Date();
    d.setHours(d.getHours() + 1, 0, 0, 0);
    return `${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}`;
  }

  /** What a run is up to: a label and a class (ok, err, live, muted). */
  function runState(run) {
    if (run.error) return ['could not start', 'err'];
    const t = run.turn;
    if (!t) return [run.conversation_id ? 'no response' : 'deleted', 'muted'];
    if (ACTIVE.has(t.status)) return [t.status, 'live'];
    if (t.status === 'completed') return ['completed', 'ok'];
    return [t.status, 'err'];
  }

  function chip(run) {
    const [label, cls] = runState(run);
    const c = el('span', `cedar-sched-chip ${cls}`, label);
    if (cls === 'live') c.prepend(el('span', 'spinner'));
    return c;
  }

  function openConversation(id) {
    if (settings?.open) settings.close();
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
    const title = el('h3', '', 'Schedule this agent');
    title.id = 'cedar-sched-title';
    head.append(title, button('Close', () => dialog.close()));

    // Stage 1: the conversation is being compacted.
    const review = el('div', 'cedar-sched-review');
    const reviewText = el('span', '', 'Reviewing the conversation…');
    const retry = button('Try again', () => compactNow());
    retry.hidden = true;
    review.append(el('span', 'spinner'), reviewText, retry);

    // Stage 2: the form.
    const f = el('form', 'cedar-sched-form');
    const titleInput = el('input');
    titleInput.maxLength = 200;
    titleInput.required = true;
    titleInput.placeholder = 'What this agent does';
    const prompt = el('textarea');
    prompt.rows = 9;
    prompt.required = true;
    prompt.placeholder = 'The prompt the agent runs each time';
    const promptHelp = el('div', 'cedar-sched-help');
    const regenerate = button('Regenerate from the conversation', () => compactNow());
    promptHelp.append(el('span', '', 'Cedar wrote this from the conversation. Edit it until it stands on its own.'), regenerate);
    const frequency = el('select');
    for (const [value, label] of FREQUENCIES) frequency.append(new Option(label, value));
    const weekday = el('select');
    DAYS.forEach((d, i) => weekday.append(new Option(d, String(i))));
    const weekdayField = field('Weekday', weekday);
    const time = el('input');
    time.type = 'time';
    time.required = true;
    const zone = el('div', 'cedar-sched-zone');
    const model = el('select');
    const modelField = field('Model', model);
    const reasoning = el('select');
    const reasoningField = field('Reasoning', reasoning);
    cedarModels.bindPair(model, reasoning);
    model.addEventListener('change', () => { reasoningField.hidden = reasoning.hidden; });
    const error = el('div', 'cedar-sched-error');
    error.hidden = true;
    const actions = el('div', 'cedar-sched-actions');
    const cancel = button('Cancel', () => dialog.close());
    const submit = el('button', 'primary', 'Schedule');
    submit.type = 'submit';
    actions.append(cancel, submit);

    const row = el('div', 'cedar-sched-row');
    row.append(field('Frequency', frequency), weekdayField, field('Time', time));
    f.append(field('Name', titleInput), field('Prompt', prompt), promptHelp, row, zone, modelField, reasoningField, error, actions);
    frequency.addEventListener('change', () => { weekdayField.hidden = frequency.value !== 'weekly'; });
    f.addEventListener('submit', (e) => { e.preventDefault(); submitForm(); });

    dialog.append(head, review, f);
    dialog.addEventListener('close', () => { editing = null; });
    document.body.append(dialog);
    return { dialog, title, review, reviewText, retry, form: f, titleInput, prompt, promptHelp, frequency, weekday, weekdayField, time, zone, model, modelField, reasoning, reasoningField, error, submit };
  }

  function showStage(stage) {
    form.review.hidden = stage !== 'review';
    form.form.hidden = stage !== 'form';
  }

  function fillModels(selected, level) {
    const shown = cedarModels.fillSelect(form.model, { selected: selected || '' });
    form.model.hidden = false;
    form.modelField.hidden = !shown;
    form.reasoningField.hidden = !cedarModels.fillLevels(form.reasoning, form.model.value, level || '');
  }

  function setTiming({ frequency = 'daily', weekday = null, time = defaultTime(), timezone = localZone() } = {}) {
    form.frequency.value = frequency;
    form.weekday.value = String(weekday ?? (new Date().getDay() + 6) % 7);
    form.weekdayField.hidden = frequency !== 'weekly';
    form.time.value = time;
    form.zone.textContent = `Times are in ${timezone}.`;
    form.zone.dataset.zone = timezone;
  }

  /** Start scheduling the open conversation: compact it, then show the form. */
  function openCreate(conversation, { model, reasoning } = {}) {
    if (!deps || !conversation) return;
    form ??= buildForm();
    editing = { conversation };
    form.title.textContent = 'Schedule this agent';
    form.submit.textContent = 'Schedule';
    form.promptHelp.hidden = false;
    form.titleInput.value = conversation.title || '';
    form.prompt.value = '';
    form.error.hidden = true;
    fillModels(model || conversation.model || '', reasoning ?? conversation.reasoning ?? '');
    setTiming();
    if (!form.dialog.open) form.dialog.showModal();
    compactNow();
  }

  /** Edit an existing schedule from Settings. */
  function openEdit(schedule) {
    if (!deps) return;
    form ??= buildForm();
    editing = { schedule };
    form.title.textContent = 'Edit scheduled agent';
    form.submit.textContent = 'Save';
    form.promptHelp.hidden = true;
    form.titleInput.value = schedule.title;
    form.prompt.value = schedule.prompt;
    form.error.hidden = true;
    fillModels(schedule.model || '', schedule.reasoning || '');
    setTiming({ frequency: schedule.frequency, weekday: schedule.weekday, time: schedule.time_of_day, timezone: schedule.timezone });
    showStage('form');
    if (!form.dialog.open) form.dialog.showModal();
    form.titleInput.focus();
  }

  async function compactNow() {
    const conversation = editing?.conversation;
    if (!conversation) return;
    showStage('review');
    form.retry.hidden = true;
    form.reviewText.textContent = 'Reviewing the conversation…';
    const id = ++request;
    try {
      const { prompt } = await deps.api('POST', `/conversations/${conversation.id}/compact`);
      if (id !== request || editing?.conversation !== conversation) return;
      form.prompt.value = prompt;
      showStage('form');
      form.prompt.focus();
    } catch (e) {
      if (id !== request || editing?.conversation !== conversation) return;
      form.reviewText.textContent = e.message || 'Could not review the conversation.';
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
    form.error.hidden = true;
    form.submit.disabled = true;
    try {
      let schedule;
      if (editing.schedule) {
        schedule = await deps.api('PATCH', `/schedules/${editing.schedule.id}`, body);
        deps.toast(`Saved · next run ${when(schedule.next_run_at)}`);
      } else {
        body.conversation_id = editing.conversation.id;
        schedule = await deps.api('POST', '/schedules', body);
        deps.toast(`Scheduled · next run ${when(schedule.next_run_at)}`);
      }
      form.dialog.close();
      if (settings?.open) render();
    } catch (e) {
      form.error.textContent = e.message || 'Could not save the schedule.';
      form.error.hidden = false;
    } finally {
      form.submit.disabled = false;
    }
  }

  // -- the Settings section ----------------------------------------------------------------------

  function stopRefresh() {
    clearTimeout(refreshTimer);
    refreshTimer = null;
  }

  /** Draw the current view (the list, or one schedule's runs) into the Settings section. */
  async function render() {
    if (!section) return;
    stopRefresh();
    const id = ++request;
    try {
      if (view && view.detail) {
        const detail = await deps.api('GET', `/schedules/${view.detail}`);
        if (id !== request) return;
        renderDetail(detail);
      } else {
        const { schedules } = await deps.api('GET', '/schedules');
        if (id !== request) return;
        renderList(schedules);
      }
    } catch (e) {
      if (id !== request) return;
      section.replaceChildren(heading('Scheduled agents'), el('div', 'empty-list', e.message || 'Could not load the schedules.'));
    }
  }

  function heading(text, back) {
    const h = el('div', 'cedar-sched-heading');
    if (back) h.append(button('← All schedules', () => { view = 'list'; render(); }));
    h.append(el('span', '', text));
    return h;
  }

  function renderList(schedules) {
    const box = el('div', 'cedar-sched-list');
    if (!schedules.length) {
      box.append(el('div', 'empty-list', 'No scheduled agents yet. Finish a chat and press Schedule to run it on its own.'));
    }
    for (const s of schedules) {
      const item = el('div', `cedar-sched-item${s.enabled ? '' : ' paused'}`);
      const main = el('div', 'cedar-sched-item-main');
      const name = el('strong', '', s.title);
      main.append(name, el('div', 'cedar-sched-sub', describe(s)));
      const next = el('div', 'cedar-sched-sub');
      if (!s.enabled) next.append(el('span', 'cedar-sched-chip muted', 'paused'));
      else next.textContent = `Next run ${when(s.next_run_at)}`;
      main.append(next);
      if (s.last_run) {
        const last = el('div', 'cedar-sched-sub');
        last.append('Last run ', when(s.last_run.started_at), ' ', chip(s.last_run));
        main.append(last);
      }
      const actions = el('div', 'cedar-sched-item-actions');
      actions.append(
        button('Runs', () => { view = { detail: s.id }; render(); }),
        button('Run now', (e) => runNow(s, e.currentTarget)),
        button(s.enabled ? 'Pause' : 'Resume', () => setEnabled(s, !s.enabled)),
        button('Edit', () => openEdit(s)),
        button('Delete', () => remove(s)),
      );
      item.append(main, actions);
      box.append(item);
    }
    section.replaceChildren(heading('Scheduled agents'), box);
  }

  function renderDetail({ schedule: s, runs }) {
    const box = el('div', 'cedar-sched-detail');
    const main = el('div', 'cedar-sched-item-main');
    main.append(el('strong', '', s.title), el('div', 'cedar-sched-sub', describe(s)));
    main.append(el('div', 'cedar-sched-sub', s.enabled ? `Next run ${when(s.next_run_at)}` : 'Paused'));
    const promptBox = el('details', 'cedar-sched-prompt');
    promptBox.append(el('summary', '', 'Prompt'), el('pre', '', s.prompt));
    const actions = el('div', 'cedar-sched-item-actions');
    actions.append(
      button('Run now', (e) => runNow(s, e.currentTarget)),
      button(s.enabled ? 'Pause' : 'Resume', () => setEnabled(s, !s.enabled)),
      button('Edit', () => openEdit(s)),
      button('Delete', () => remove(s)),
    );
    box.append(main, promptBox, actions, el('div', 'cedar-sched-runs-label', `${runs.length} run${runs.length === 1 ? '' : 's'}`));
    if (!runs.length) box.append(el('div', 'empty-list', 'No runs yet.'));
    let live = false;
    for (const run of runs) {
      const [, cls] = runState(run);
      live ||= cls === 'live';
      box.append(runRow(run));
    }
    section.replaceChildren(heading(s.title, true), box);
    if (live && settings?.open) refreshTimer = setTimeout(render, REFRESH_MS);
  }

  function runRow(run) {
    const row = el('div', 'cedar-sched-run');
    const head = el('div', 'cedar-sched-run-head');
    head.append(el('span', 'cedar-sched-when', when(run.started_at)), chip(run));
    if (run.manual) head.append(el('span', 'cedar-sched-chip muted', 'run now'));
    if (run.conversation_id) head.append(button('Open', () => openConversation(run.conversation_id)));
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
        const more = button('Show all', () => { md.innerHTML = deps.renderMarkdown(text); more.remove(); });
        body.append(more);
      }
    } else if (run.turn && ACTIVE.has(run.turn.status)) {
      body.append(el('div', 'cedar-sched-sub', 'Working… open it to follow along.'));
    }
    if (body.childElementCount) row.append(body);
    return row;
  }

  async function runNow(s, btn) {
    if (btn) btn.disabled = true;
    try {
      await deps.api('POST', `/schedules/${s.id}/run`);
      deps.toast(`Started “${s.title}”`);
      view = { detail: s.id };
      render();
    } catch (e) {
      deps.toast(e.message);
      if (btn) btn.disabled = false;
    }
  }

  async function setEnabled(s, enabled) {
    try {
      await deps.api('PATCH', `/schedules/${s.id}`, { enabled });
      render();
    } catch (e) {
      deps.toast(e.message);
    }
  }

  async function remove(s) {
    if (!confirm(`Delete “${s.title}” and the results of all its runs? This cannot be undone.`)) return;
    try {
      await deps.api('DELETE', `/schedules/${s.id}`);
      view = 'list';
      render();
    } catch (e) {
      deps.toast(e.message);
    }
  }

  function bindSettings() {
    section = document.getElementById('schedules-section');
    settings = document.getElementById('settings-dialog');
    const open = document.getElementById('settings-btn');
    if (!section || !settings || !open) return;
    // ui-pref.js opens the dialog on this click; the list is (re)loaded each time it opens.
    open.addEventListener('click', () => { view = 'list'; render(); });
    settings.addEventListener('close', stopRefresh);
    window.addEventListener('hashchange', () => { if (form?.dialog.open) form.dialog.close(); });
  }

  function init(helpers) {
    deps = helpers;
    if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', bindSettings);
    else bindSettings();
  }

  return { init, openCreate, openEdit, describe };
})();

'use strict';

// Clarifying questions for cedar's `ask_user` tool, shared by both UIs (/v1 and /v2). The tool's
// result is a JSON object, {"questions": [{"question", "header"?, "multi_select", "options":
// [{"label", "description"?}]}], "note"}, and this turns it into a form: each question's options as
// radio buttons (one answer) or checkboxes (multi_select), always with a free-form answer of the
// user's own, and a button that sends the answers as the next message:
//
//   cedarClarify.parse(output) -> spec | null           the validated result, or null if it isn't one
//   cedarClarify.element(spec, { onSubmit, answered })  the form; onSubmit(text) sends the answer and
//                                                       returns a promise (a rejection re-opens it)
//   cedarClarify.settle(root)                           close every open form under root, once a
//                                                       later message has answered it
//
// It needs nothing from the app. Everything in the result came from the model, so text is only ever
// set as text.

const cedarClarify = (() => {
  const MAX_QUESTIONS = 4;
  const MAX_OPTIONS = 8;
  const MAX_QUESTION = 500;
  const MAX_HEADER = 40;
  const MAX_LABEL = 100;
  const MAX_DESCRIPTION = 200;
  let forms = 0; // gives each form's radio groups names of their own

  function el(tag, className, text) {
    const e = document.createElement(tag);
    if (className) e.className = className;
    if (text !== undefined) e.textContent = text;
    return e;
  }

  const str = (v, max) => (typeof v === 'string' && v.trim() ? v.trim().slice(0, max) : '');

  /** The questions in an `ask_user` result, or null if `output` isn't one with at least one question. */
  function parse(output) {
    let data;
    try { data = JSON.parse(output); } catch { return null; }
    if (!data || typeof data !== 'object' || !Array.isArray(data.questions)) return null;
    const questions = data.questions.slice(0, MAX_QUESTIONS).map((q) => {
      const options = (Array.isArray(q?.options) ? q.options : []).slice(0, MAX_OPTIONS)
        .map((o) => ({ label: str(o?.label, MAX_LABEL), description: str(o?.description, MAX_DESCRIPTION) }))
        .filter((o) => o.label);
      return {
        question: str(q?.question, MAX_QUESTION),
        header: str(q?.header, MAX_HEADER),
        multi: q?.multi_select === true && options.length > 0,
        options,
      };
    }).filter((q) => q.question);
    return questions.length ? { questions } : null;
  }

  /**
   * The answers as the message that goes back to the model: each question, then what was chosen
   * and written. A question left alone says so, so the model doesn't take silence for a choice.
   */
  function answerText(spec, answers) {
    const lines = spec.questions.map((q, i) => {
      const { chosen, own } = answers[i];
      const parts = [...chosen];
      if (own) parts.push(chosen.length || q.options.length ? tr('clarify.otherAnswer', { text: own }) : own);
      const answer = parts.length ? parts.join(', ') : tr('clarify.noPreference');
      return spec.questions.length === 1 ? `${q.question}\n${answer}` : `${i + 1}. ${q.question}\n${answer}`;
    });
    return lines.join('\n\n');
  }

  /** One question as a fieldset; `read()` returns what is chosen and written. */
  function questionField(q, name) {
    const field = el('fieldset', 'cedar-clarify-q');
    const legend = el('legend', 'cedar-clarify-question');
    if (q.header) legend.append(el('span', 'cedar-clarify-chip', q.header));
    legend.append(el('span', '', q.question));
    field.append(legend);

    const type = q.multi ? 'checkbox' : 'radio';
    const boxes = [];
    if (q.options.length) {
      field.append(el('div', 'cedar-clarify-hint', tr(q.multi ? 'clarify.chooseAny' : 'clarify.chooseOne')));
      const list = el('div', 'cedar-clarify-options');
      for (const o of q.options) {
        const row = el('label', 'cedar-clarify-opt');
        const box = el('input');
        box.type = type;
        box.name = name;
        box.value = o.label;
        const text = el('span', 'cedar-clarify-opt-text');
        text.append(el('span', 'cedar-clarify-opt-label', o.label));
        if (o.description) text.append(el('span', 'cedar-clarify-opt-desc', o.description));
        row.append(box, text);
        list.append(row);
        boxes.push(box);
      }
      field.append(list);
    }

    // The free-form answer: an "Other" choice beside the options, or the whole answer to an open
    // question. Writing in it picks it, and emptying it lets it go.
    let other = null;
    let own;
    if (q.options.length) {
      const row = el('div', 'cedar-clarify-opt cedar-clarify-other');
      other = el('input');
      other.type = type;
      other.name = name;
      other.setAttribute('aria-label', tr('clarify.other'));
      own = el('input', 'cedar-clarify-own');
      own.type = 'text';
      own.maxLength = 1000;
      own.placeholder = tr('clarify.otherPlaceholder');
      own.setAttribute('aria-label', tr('clarify.ownAria', { question: q.question }));
      own.addEventListener('input', () => { other.checked = own.value.trim() !== ''; field.dispatchEvent(new Event('change', { bubbles: true })); });
      other.addEventListener('change', () => { if (other.checked && !own.value.trim()) own.focus(); });
      row.append(other, own);
      field.querySelector('.cedar-clarify-options').append(row);
      boxes.push(other);
    } else {
      own = el('textarea', 'cedar-clarify-own');
      own.rows = 2;
      own.maxLength = 2000;
      own.placeholder = tr('clarify.yourAnswer');
      own.setAttribute('aria-label', q.question);
      field.append(own);
    }

    const read = () => ({
      chosen: boxes.filter((b) => b.checked && b !== other).map((b) => b.value),
      own: (!other || other.checked) ? own.value.trim() : '',
    });
    return { field, read };
  }

  /** Close a form: nothing in it can change any more. */
  function close(root, label) {
    root.classList.add('answered');
    for (const input of root.querySelectorAll('input, textarea, button')) input.disabled = true;
    const state = root.querySelector('.cedar-clarify-state');
    if (state) state.textContent = label;
  }

  /** The form for `spec`. Without `onSubmit`, or with `answered`, it is shown closed. */
  function element(spec, { onSubmit, answered = false } = {}) {
    const n = ++forms;
    const root = el('div', 'cedar-clarify');
    const head = el('div', 'cedar-clarify-head');
    head.append(
      el('span', 'cedar-clarify-title', tr('clarify.title', { count: spec.questions.length })),
      el('span', 'cedar-clarify-state', ''),
    );
    const form = el('form', 'cedar-clarify-form');
    const fields = spec.questions.map((q, i) => questionField(q, `cedar-clarify-${n}-${i}`));
    for (const f of fields) form.append(f.field);

    const foot = el('div', 'cedar-clarify-foot');
    const send = el('button', 'cedar-clarify-send', tr('clarify.send'));
    send.type = 'submit';
    foot.append(send, el('span', 'cedar-clarify-alt', tr('clarify.alt')));
    form.append(foot);
    root.append(head, form);

    const answers = () => fields.map((f) => f.read());
    const any = () => answers().some((a) => a.chosen.length || a.own);
    const sync = () => { if (!root.classList.contains('answered')) send.disabled = !any(); };
    form.addEventListener('change', sync);
    form.addEventListener('input', sync);
    // Enter in a one-line answer sends the form, as long as something is answered; Ctrl/⌘+Enter
    // does the same from a multi-line one.
    form.addEventListener('keydown', (e) => {
      if (e.key !== 'Enter' || e.isComposing) return;
      if (e.target.tagName === 'TEXTAREA' && !(e.ctrlKey || e.metaKey)) return;
      if (e.target.type === 'radio' || e.target.type === 'checkbox' || e.target.tagName === 'BUTTON') return;
      e.preventDefault();
      if (!send.disabled) form.requestSubmit();
    });
    form.addEventListener('submit', async (e) => {
      e.preventDefault();
      if (root.classList.contains('answered') || !onSubmit || !any()) return;
      close(root, tr('clarify.sending'));
      try {
        await onSubmit(answerText(spec, answers()));
        close(root, tr('clarify.answered'));
      } catch {
        // The app has said what went wrong; the answers stay so they can be sent again.
        root.classList.remove('answered');
        for (const input of root.querySelectorAll('input, textarea, button')) input.disabled = false;
        root.querySelector('.cedar-clarify-state').textContent = '';
        sync();
      }
    });

    if (answered || !onSubmit) close(root, tr('clarify.answered'));
    else sync();
    return root;
  }

  /** Close the open forms under `root`: a later message has answered them. */
  function settle(root) {
    for (const form of root.querySelectorAll('.cedar-clarify:not(.answered)')) close(form, tr('clarify.answered'));
  }

  return { parse, element, settle, answerText };
})();

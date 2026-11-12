'use strict';

// A follow-up the model suggested (cedar's `suggest_follow_up` tool), shared by every UI. The
// tool schedules nothing; its result is a JSON object, {"follow_up": {"title", "prompt", "at"},
// "note"}, and this draws it as a card with two buttons. Setting it up makes a one-off agent on
// the server (PUT /api/turns/{turn}/followups/{call}); dismissing it records that it was seen.
// The first decision stands, on every device.
//
//   cedarFollowUp.init({ api, toast })                    the app's /api client and its notices
//   cedarFollowUp.known(followups)                        the decisions a conversation already has
//                                                         (GET /api/conversations/{id})
//   cedarFollowUp.parse(output) -> spec | null            the validated result, or null
//   cedarFollowUp.element(spec, { turnId, callId })       the card
//
// Everything in the result came from the model, so text is only ever set as text.

const cedarFollowUp = (() => {
  const MAX_TITLE = 200;
  const MAX_PROMPT = 8000;
  let deps = null;
  const decided = new Map(); // "turn/call" -> { status, agent_id }

  function el(tag, className, text) {
    const e = document.createElement(tag);
    if (className) e.className = className;
    if (text !== undefined) e.textContent = text;
    return e;
  }

  const str = (v, max) => (typeof v === 'string' && v.trim() ? v.trim().slice(0, max) : '');

  /** The suggestion in a `suggest_follow_up` result, or null if `output` isn't one. */
  function parse(output) {
    let data;
    try { data = JSON.parse(output); } catch { return null; }
    const f = data?.follow_up;
    if (!f || typeof f !== 'object') return null;
    const title = str(f.title, MAX_TITLE);
    const prompt = str(f.prompt, MAX_PROMPT);
    const at = new Date(f.at);
    if (!title || !prompt || Number.isNaN(at.getTime())) return null;
    return { title, prompt, at, atText: f.at };
  }

  function localZone() {
    try { return Intl.DateTimeFormat().resolvedOptions().timeZone || 'UTC'; } catch { return 'UTC'; }
  }

  /** Take in what was already decided in the conversation being opened. */
  function known(followups) {
    for (const f of followups || []) decided.set(`${f.turn_id}/${f.call_id}`, f);
  }

  function element(spec, { turnId, callId }) {
    const key = `${turnId}/${callId}`;
    const card = el('div', 'cedar-followup');
    const when = spec.at.toLocaleString(cedarI18n.locale, { weekday: 'long', day: 'numeric', month: 'long', hour: '2-digit', minute: '2-digit' });
    const head = el('div', 'cedar-followup-head');
    head.append(el('b', '', spec.title), el('span', '', tr('followup.when', { when })));
    const what = el('details', 'cedar-followup-prompt');
    what.append(el('summary', '', tr('followup.prompt')), el('pre', '', spec.prompt));
    const foot = el('div', 'cedar-followup-foot');
    card.append(head, what, foot);

    const draw = () => {
      const done = decided.get(key);
      foot.textContent = '';
      card.classList.toggle('settled', !!done);
      if (done?.status === 'accepted') {
        foot.append(el('span', 'cedar-followup-done', tr('followup.accepted')));
        if (done.agent_id) {
          const link = el('a', 'copy-btn', tr('followup.openAgent'));
          link.href = `#/agents/${done.agent_id}`;
          foot.append(link);
        }
        return;
      }
      if (done) {
        foot.append(el('span', 'cedar-followup-note', tr('followup.dismissed')));
        return;
      }
      const past = spec.at.getTime() <= Date.now();
      const act = (label, cls, body) => {
        const b = el('button', cls, label);
        b.type = 'button';
        b.addEventListener('click', async () => {
          for (const other of foot.querySelectorAll('button')) other.disabled = true;
          try {
            const { followup } = await deps.api('PUT', `/turns/${turnId}/followups/${encodeURIComponent(callId)}`, body);
            decided.set(key, followup);
          } catch (e) {
            deps.toast(e.message || tr('followup.failed'));
          }
          draw();
        });
        return b;
      };
      foot.append(el('span', 'cedar-followup-note', past ? tr('followup.passed') : tr('followup.note')));
      if (!past) foot.append(act(tr('followup.setUp'), 'primary', { decision: 'accept', title: spec.title, prompt: spec.prompt, at: spec.atText, timezone: localZone() }));
      foot.append(act(tr('followup.dismiss'), 'copy-btn', { decision: 'dismiss' }));
    };
    draw();
    return card;
  }

  function init(helpers) {
    deps = helpers;
  }

  return { init, known, parse, element };
})();

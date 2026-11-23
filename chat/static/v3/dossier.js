'use strict';

// The "Dossier" layout of cedar-chat's main page (/v4/): one column, and each turn is a brief that
// reads top to bottom. The prompt; the Plan Cedar committed to; the Work, a timeline of its
// reasoning, one row per tool call with the call's output folded under it, Cedar's interim notes
// and any approval it had to wait for; the Result, which is the document it wrote, its maps,
// charts and images, and its closing note, with [n] linked to the Sources listed last. The Work
// block stays open while the run is live and folds to one line when it is over. The sidebar lists
// the conversations and outlines the newest brief.
//
// app.js holds everything the layouts share and calls the functions returned here. They use its
// globals (els, state, h, t, tr, note, fillStep, ...) freely: this file is loaded first, but
// nothing in it runs until app.js calls it.

(window.cedarLayouts ||= {}).dossier = (() => {
  /** The words this layout says differently are `v3.dossier.*`; the rest are the page's. */
  const OWN = new Set([
    'example2.text', 'example3.prompt', 'example3.text', 'greeting.afternoon', 'greeting.evening', 'greeting.morning',
    'hero.title', 'placeholder.new', 'placeholder.reply', 'you',
  ]);
  const key = (name) => (OWN.has(name) ? `v3.dossier.${name}` : `v3.${name}`);

  // -- a turn: one brief ---------------------------------------------------------------------------

  /** A block of the brief: a label column (name, a note, actions) and a body. */
  function block(name, cls) {
    const el = h('section', `block ${cls}`);
    const label = h('div', 'block-label');
    label.append(t('span', 'name', name), t('span', 'sub', ''));
    const body = h('div', 'block-body');
    el.append(label, body);
    el.hidden = true;
    return el;
  }
  const blockBody = (el) => el.querySelector('.block-body');
  const blockSub = (el, text) => { el.querySelector('.sub').textContent = text; };

  function startTurn(view, turn) {
    const brief = h('section', 'brief');
    brief.dataset.turn = turn.id;
    brief.dataset.screenLabel = `Turn ${state.order.length}`;
    const prompt = h('div', 'prompt');
    prompt.append(t('div', 'who', tr('v3.dossier.you', { day: new Date(turn.created_at || Date.now()).toLocaleDateString(cedarI18n.locale, { weekday: 'long' }), time: fmtTime(turn.created_at) })));
    prompt.lastChild.append(cedarTrace.button(turn.id), cedarActions.button(turn));
    const text = h('p', 'text');
    if (turn.attachments?.length) text.append(cedarAttach.chips(turn.attachments));
    text.append(document.createTextNode(turn.prompt));
    prompt.append(text);
    brief.append(prompt);
    view.blocks = { plan: block(tr('v3.block.plan'), 'plan'), work: block(tr('v3.block.work'), 'work'), result: block(tr('v3.block.result'), 'result'), sources: block(tr('v3.block.sources'), 'sources') };
    const foot = h('div', 'foot');
    foot.hidden = true;
    brief.append(view.blocks.plan, view.blocks.work, view.blocks.result, view.blocks.sources, foot);
    view.brief = brief;
    view.foot = foot;
    view.workBody = blockBody(view.blocks.work);
    view.resultBody = blockBody(view.blocks.result);
    // The Work block folds from its label.
    const toggle = t('button', 'linkish', tr('v3.show'));
    toggle.type = 'button';
    toggle.addEventListener('click', () => { view.blocks.work.classList.toggle('folded'); toggle.textContent = view.blocks.work.classList.contains('folded') ? tr('v3.show') : tr('v3.hide'); });
    view.blocks.work.querySelector('.block-label').append(toggle);
    view.workToggle = toggle;
    view.blocks.work.hidden = false;
    cedarClarify.settle(els.briefs); // this message answers any question still open above it
    els.briefs.append(brief);
    view.workBody.append(view.workingEl);
  }

  /** The Result's label column: Download / Copy / View for the document the turn wrote. */
  function setResultActions(view, actions, name) {
    const label = view.blocks.result.querySelector('.block-label');
    label.querySelector('.acts')?.remove();
    if (name) blockSub(view.blocks.result, name);
    if (!actions?.length) return;
    const acts = h('div', 'acts');
    acts.append(...actions);
    label.append(acts);
  }

  /** "5 steps · 48 s" under the Work label, or what is running. */
  function updateWorkLabel(view) {
    const running = view.tools.find((tool) => tool.output === undefined);
    const n = view.tools.length;
    if (!view.done) blockSub(view.blocks.work, running ? tr('v3.work.step', { n, doing: toolText(running, false) }) : n ? tr('v3.work.stepsSoFar', { count: n }) : tr('v3.work.starting'));
    else {
      const secs = Math.max(1, Math.round((view.endedAt - view.startedAt) / 1000));
      const failed = view.tools.filter((tool) => tool.is_error).length;
      const took = secs < 90 ? tr('v3.duration.seconds', { n: secs }) : tr('v3.duration.minutes', { n: Math.round(secs / 60) });
      blockSub(view.blocks.work, `${tr('v3.count.steps', { count: n })} · ${took}${failed ? ` · ${tr('v3.count.failed', { count: failed })}` : ''}`);
    }
  }

  /** An entry of the Work timeline, stamped with the time it landed. */
  function placeNote(view, el, at) {
    const follow = nearEnd(els.scroll) || view === state.activeTurn;
    const entry = h('div', 'entry');
    entry.append(t('span', 'at', fmtTime(at)), el);
    if (el.classList.contains('steps') || el.classList.contains('cedar-approval')) entry.classList.add('has-card');
    if (view.workingEl?.isConnected) view.workBody.insertBefore(entry, view.workingEl); else view.workBody.append(entry);
    if (follow) els.scroll.scrollTop = els.scroll.scrollHeight;
  }

  /** Put a card where it belongs in the brief: a document, widget or figures in the Result, the
   *  folded output of a search, command or read under its step row in the Work. */
  function placeCard(view, card) {
    const follow = nearEnd(els.scroll, 240) || view === state.activeTurn;
    const kind = card.classList.contains('figures') ? 'figures' : cardKind(card.cedarTool || {});
    if (kind === 'doc' || kind === 'widget' || kind === 'figures') {
      // The document leads the Result; widgets and figures follow it.
      if (kind === 'doc') view.resultBody.prepend(card); else view.resultBody.append(card);
      view.blocks.result.hidden = false;
    } else if (card.cedarTool?.step) {
      card.cedarTool.step.after(card);
    } else {
      view.workBody.insertBefore(card, view.workingEl);
    }
    if (follow) els.scroll.scrollTop = els.scroll.scrollHeight;
    renderOutline();
  }

  /** A step row's card: fold a trace open or closed under the row; scroll to a result. */
  function focusCard(tool) {
    const card = tool.card;
    if (!card || card.hidden) return;
    if (card.classList.contains('trace')) {
      tool.open = !tool.open;
      card.classList.toggle('open', tool.open);
      card.querySelector('.card-head')?.setAttribute('aria-expanded', String(tool.open));
      return;
    }
    card.scrollIntoView({ block: 'center', behavior: narrow.matches ? 'auto' : 'smooth' });
    card.classList.remove('flash');
    requestAnimationFrame(() => card.classList.add('flash'));
  }

  /** A step row for a tool call, in the conversation, that jumps to its card. */
  function addStep(view, tool) {
    if (!view.stepsEl) {
      view.stepsEl = h('div', 'steps');
      placeNote(view, view.stepsEl);
    }
    const row = h('button', 'step running');
    row.type = 'button';
    row.append(t('span', 'i', String(view.tools.length)), h('span', 'what'), t('span', 'ms', ''));
    row.addEventListener('click', () => focusCard(tool));
    tool.step = row;
    view.stepsEl.append(row);
    fillStep(tool);
    updateWorkLabel(view);
    if (nearEnd(els.scroll, 200) || view === state.activeTurn) els.scroll.scrollTop = els.scroll.scrollHeight;
    renderOutline();
  }

  const stepFilled = (tool) => updateWorkLabel(tool.view);

  /** In the brief the document is paper: its actions move to the Result's label column. */
  const documentFilled = (tool, actions, name) => { if (tool.view) setResultActions(tool.view, actions, name); };

  // -- the plan, decisions and sources -------------------------------------------------------------

  function showPlan(view, data, awaiting) {
    const plan = cedarTrust.plan.parse(data);
    if (!plan) return;
    view.plan = plan;
    const el = cedarTrust.plan.element(plan, { awaiting: awaiting && !view.done, onApprove: (steps) => approvePlan(view, steps) });
    const body = blockBody(view.blocks.plan);
    body.textContent = '';
    body.append(el);
    view.planEl = el;
    view.blocks.plan.hidden = false;
    blockSub(view.blocks.plan, awaiting && !view.done ? cedarTrust.plan.awaitingNote(plan) : tr('v3.plan.agreedAt', { time: fmtTime(data.created_at) }));
    if (awaiting && !view.done) setWorking(view, tr('v3.waitingPlan'));
    renderOutline();
  }

  /** The reader changed the steps before starting. */
  const planChanged = (view) => blockSub(view.blocks.plan, tr('v3.plan.agreedChangesAt', { time: fmtTime() }));

  /** A card that waits for a decision (a suggested follow-up, a held call, what Cedar couldn't
   *  do) goes in the Result, not the Work: the Work folds away once the turn is done. */
  function placeDecision(view, card, heading, limitation) {
    view.resultBody.append(card);
    view.blocks.result.hidden = false;
    if (limitation) (view.limitCards ||= []).push(card);
  }

  function renderSources(view) {
    const sources = view.sources = sourcesOf(view);
    const body = blockBody(view.blocks.sources);
    body.textContent = '';
    view.blocks.sources.hidden = !sources.length;
    if (!sources.length) return;
    body.append(cedarTrust.sources.element(sources, { onOpen: (s) => s.path && downloadFile(workspacePath(s.path)) }));
    blockSub(view.blocks.sources, tr('v3.sourcesRead', { count: sources.length }));
    renderOutline();
  }

  /** The sidebar's outline of the newest brief: its blocks, with where each stands. */
  function renderOutline() {
    const view = state.views.get(state.order[state.order.length - 1]);
    els.outlineWrap.hidden = !view;
    els.outline.textContent = '';
    if (!view) return;
    const item = (label, value, ok, target) => {
      const a = h('a');
      a.href = '#';
      a.append(t('span', '', label), t('span', `v${ok ? ' ok' : ''}`, value));
      a.addEventListener('click', (e) => { e.preventDefault(); els.scroll.scrollTo({ top: target.offsetTop - 16, behavior: 'smooth' }); });
      return a;
    };
    if (view.plan) {
      const done = view.planEl.querySelectorAll('.cedar-plan-step[data-state="done"]').length;
      els.outline.append(item(tr('v3.block.plan'), `${cedarI18n.number(done)}/${cedarI18n.number(view.plan.steps.length)}`, done === view.plan.steps.length, view.blocks.plan));
    }
    els.outline.append(item(tr('v3.block.work'), tr('v3.count.steps', { count: view.tools.length }), false, view.blocks.work));
    if (!view.blocks.result.hidden) {
      const doc = view.tools.find((tool) => tool.card && cardKind(tool) === 'doc');
      els.outline.append(item(tr('v3.block.result'), doc ? String(doc.input.path).split('/').pop() : '', false, view.blocks.result));
    }
    if (view.sources.length) els.outline.append(item(tr('v3.block.sources'), cedarI18n.number(view.sources.length), false, view.blocks.sources));
  }

  // -- the end of a turn ---------------------------------------------------------------------------

  /** A closing note is the last entry of the Work, stamped with when the turn ended. */
  function putClosing(view, node, anchor, at) {
    const entry = h('div', 'entry');
    entry.append(t('span', 'at', fmtTime(at)), node);
    if (anchor?.isConnected) view.workBody.insertBefore(entry, anchor); else view.workBody.append(entry);
  }

  function closeTurn(view, event, anchor) {
    const d = event.data || {};
    anchor?.remove();
    // Cedar's closing note opens the Result; what it said along the way stays in the Work.
    const last = view.textNotes[view.textNotes.length - 1];
    if (last && event.type === 'run.completed' && d.stop_reason !== 'ask_user') {
      last.parentElement.remove();
      view.resultBody.prepend(last);
      view.blocks.result.hidden = false;
    }
    view.planEl?.cedarPlan.finish(event.type === 'run.completed');
    if (view.planEl) blockSub(view.blocks.plan, event.type === 'run.completed' ? tr('v3.plan.done') : tr('v3.plan.stoppedEarly'));
    updateWorkLabel(view);
    const failed = event.type === 'run.failed' || view.tools.some((tool) => tool.is_error);
    view.blocks.work.classList.toggle('folded', !failed);
    view.workToggle.textContent = failed ? tr('v3.hide') : tr('v3.show');
    renderSources(view);
    view.foot.textContent = '';
    view.foot.append(t('span', '', cedarTrust.sandbox.text(view.started, view.approvalsGranted)));
    if (view.usage.input || view.usage.output) view.foot.append(t('span', 'usage', cedarTrust.cost.text(view.usage)));
    view.foot.hidden = false;
    renderOutline();
  }

  /** Once a watched turn is over, the page rests on the top of its Result; unless the reply says
   *  what Cedar couldn't do, which ends on the card that offers what to do about it: then on that. */
  function restOnResult(view) {
    const card = view.limitCards?.at(-1);
    const target = view.blocks.result.hidden ? view.blocks.work : view.blocks.result;
    if (card) card.scrollIntoView({ block: 'end', behavior: 'smooth' });
    else els.scroll.scrollTo({ top: target.offsetTop - 16, behavior: 'smooth' });
  }

  function clearTurns() {
    els.briefs.textContent = '';
    els.outline.textContent = '';
    els.outlineWrap.hidden = true;
  }

  // -- chrome --------------------------------------------------------------------------------------

  function chromeChanged(c, total) {
    els.send.querySelector('.send-lbl').textContent = c ? tr('v3.send') : tr('v3.planIt');
    // The composer lives in the column on the welcome screen and in the footer once a brief is open.
    if (!c && els.composer.parentElement !== els.hero.parentElement) { els.hero.after(els.composer); els.composer.classList.add('in-column'); }
    else if (c && els.composer.parentElement !== els.slip) { els.slip.append(els.composer); els.composer.classList.remove('in-column'); }
    els.cost.textContent = c && (total.input || total.output) ? cedarTrust.cost.text(total) : '';
    if (!c) renderOutline();
  }

  // -- the sidebar, a drawer on a narrow screen ----------------------------------------------------

  const drawerOpen = () => els.drawer.classList.contains('open');

  function openDrawer() {
    if (!narrow.matches) return;
    els.drawer.classList.add('open');
    els.backdrop.hidden = false;
    els.chatsBtn.setAttribute('aria-expanded', 'true');
    renderRecents();
    (els.recents.querySelector('.conv.active') || els.recents.querySelector('.conv') || els.drawerClose).focus();
  }

  function closeDrawer() {
    if (!els.drawer.classList.contains('open')) return;
    els.drawer.classList.remove('open');
    els.backdrop.hidden = true;
    els.chatsBtn.setAttribute('aria-expanded', 'false');
  }

  return {
    narrow: '(max-width: 860px)',
    key,
    scrollers: () => ({ notes: els.scroll, cards: els.scroll }),
    startTurn, placeNote, placeCard, focusCard, addStep, stepFilled, documentFilled,
    showPlan, planChanged, placeDecision, renderSources, putClosing, closeTurn, restOnResult,
    chromeChanged, conversationsChanged: () => {}, clearTurns,
    setPane: () => {}, updateTabs: () => {}, // one column: no panes
    openDrawer, closeDrawer, drawerOpen,
    narrowChanged: () => { if (!narrow.matches) closeDrawer(); },
    bindLayout: () => {},
  };
})();

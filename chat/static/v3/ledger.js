'use strict';

// The "Ledger" layout of cedar-chat's main page (/v3/): say on the left, show on the right. The
// conversation (prompts, Cedar's prose, its questions, the plan, and tool calls folded into one
// "Worked through N steps" line per run of steps) is a narrow column; everything a tool produced
// is a card on the stage beside it, grouped by turn, with a rail of the sources the turn read.
// On a narrow screen the two panes become tabs.
//
// app.js holds everything the layouts share and calls the functions returned here. They use its
// globals (els, state, h, t, tr, note, fillStep, ...) freely: this file is loaded first, but
// nothing in it runs until app.js calls it.

(window.cedarLayouts ||= {}).ledger = (() => {
  const SPLIT_KEY = 'cedar-chat-v2-talk-width';

  // -- a turn ------------------------------------------------------------------------------------

  /** The prompt as a note; the turn's notes go before its working line. */
  function startTurn(view, turn) {
    view.workEl = null;   // the fold around the open run of steps ("Worked through N steps")
    view.works = [];      // every fold of this turn, to close when it ends
    view.group = null;    // this turn's group of cards on the stage
    view.cardsEl = null;
    view.sourcesEl = null;
    const user = note('user', escapeHtml(tr('v3.you', { time: fmtTime(turn.created_at) })));
    user.firstChild.append(cedarTrace.button(turn.id), cedarActions.button(turn));
    if (turn.attachments?.length) user.lastChild.append(cedarAttach.chips(turn.attachments));
    user.lastChild.append(document.createTextNode(turn.prompt));
    cedarClarify.settle(els.notes); // this message answers any question still open above it
    els.notes.append(user);
    els.notes.append(view.workingEl);
  }

  function placeNote(view, el) {
    const follow = nearEnd(els.notes) || view === state.activeTurn;
    els.notes.insertBefore(el, view.workingEl);
    if (follow) els.notes.scrollTop = els.notes.scrollHeight;
  }

  // -- the stage: one card per tool call -----------------------------------------------------------

  /** The stage group for a turn, made when its first card lands. */
  function ensureGroup(view) {
    if (view.group) return view.group;
    const group = h('section', 'turn');
    group.dataset.turn = view.turn.id;
    const n = state.order.indexOf(view.turn.id) + 1;
    const head = h('div', 'turn-head');
    head.append(t('span', 'n', tr('v3.turn', { n })), t('span', 'q', firstLine(view.turn.prompt).slice(0, 160)), t('span', 't', fmtTime(view.turn.created_at)));
    head.querySelector('.q').title = view.turn.prompt;
    group.append(head);
    const grid = h('div', 'turn-grid');
    const cards = h('div', 'turn-cards');
    const sources = h('aside', 'turn-sources');
    sources.setAttribute('aria-label', tr('v3.sources'));
    grid.append(cards, sources);
    group.append(grid);
    view.cardsEl = cards;
    view.sourcesEl = sources;
    // Turns are replayed side by side when a conversation opens, so a group goes after the groups of earlier turns.
    const later = state.order.slice(state.order.indexOf(view.turn.id) + 1).map((id) => state.views.get(id)?.group).find((g) => g?.isConnected);
    if (later) els.stageBody.insertBefore(group, later); else els.stageBody.append(group);
    view.group = group;
    els.stageEmpty.hidden = true;
    renderSources(view);
    return group;
  }

  /** Put a card on the stage for `view`, following along if the reader was at the end. */
  function placeCard(view, card) {
    const follow = nearEnd(els.stage, 240) || view === state.activeTurn;
    ensureGroup(view);
    view.cardsEl.append(card);
    if (follow) els.stage.scrollTop = els.stage.scrollHeight;
    if (narrow.matches && els.app.dataset.pane !== 'stage') { state.unseen++; updateTabs(); }
  }

  /** Scroll the stage to a tool's card (opening a folded one) and flash it. */
  function focusCard(tool) {
    const card = tool.card;
    if (!card || card.hidden) return;
    if (narrow.matches) setPane('stage');
    if (card.classList.contains('trace') && !tool.open) {
      tool.open = true;
      card.classList.add('open');
      card.querySelector('.card-head')?.setAttribute('aria-expanded', 'true');
    }
    card.scrollIntoView({ block: 'center', behavior: narrow.matches ? 'auto' : 'smooth' });
    card.classList.remove('flash');
    requestAnimationFrame(() => card.classList.add('flash'));
  }

  // -- steps, folded -------------------------------------------------------------------------------

  /** A step row for a tool call, in the conversation, that jumps to its card. */
  function addStep(view, tool) {
    if (!view.stepsEl) {
      const work = h('div', 'work live');
      const sum = h('button', 'work-sum');
      sum.type = 'button';
      sum.append(h('span', 'work-text'), t('span', 'work-toggle', tr('v3.hide')));
      sum.addEventListener('click', () => {
        work.classList.toggle('open');
        sum.lastChild.textContent = work.classList.contains('open') || work.classList.contains('live') ? tr('v3.hide') : tr('v3.show');
      });
      const steps = h('div', 'steps');
      work.append(sum, steps);
      work.cedarTools = [];
      view.stepsEl = steps;
      view.workEl = work;
      view.works.push(work);
      placeNote(view, work);
    }
    view.workEl.cedarTools.push(tool);
    tool.work = view.workEl;
    const row = h('button', 'step running');
    row.type = 'button';
    row.append(t('span', 'i', String(view.tools.length)), h('span', 'what'), t('span', 'ms', ''));
    row.addEventListener('click', () => focusCard(tool));
    tool.step = row;
    view.stepsEl.append(row);
    fillStep(tool);
    if (nearEnd(els.notes, 200) || view === state.activeTurn) els.notes.scrollTop = els.notes.scrollHeight;
  }

  const stepFilled = (tool) => { if (tool.work) updateWork(tool.work); };

  /** "Worked through 5 steps · searched twice, read 1 page, wrote 1 file", or what is running. */
  function updateWork(work) {
    const tools = work.cedarTools;
    const running = tools.find((tool) => tool.output === undefined);
    const text = work.querySelector('.work-text');
    if (running) { text.textContent = tr('v3.work.step', { n: tools.length, doing: toolText(running, false) }); return; }
    const n = (names) => tools.filter((tool) => names.includes(tool.name)).length;
    const times = (k, key) => (k === 0 ? '' : tr(key, { count: k }));
    const parts = [
      times(n(['web_search']), 'v3.work.searched'),
      times(n(['fetch_url', 'download_file', 'read_file', 'read_document']), 'v3.work.read'),
      times(n(['bash', 'run_python']), 'v3.work.ran'),
      times(n(['write_file', 'generate_pdf', 'generate_chart']), 'v3.work.wrote'),
      times(tools.filter((tool) => tool.is_error).length, 'v3.count.failed'),
    ].filter(Boolean);
    text.textContent = `${tr('v3.work.summary', { count: tools.length })}${parts.length ? ` · ${parts.join(', ')}` : ''}`;
  }

  /** The run is over: the folds close (a failure stays open, since it is worth reading). */
  function settleWork(view) {
    for (const work of view.works) {
      work.classList.remove('live');
      const failed = work.cedarTools.some((tool) => tool.is_error);
      work.classList.toggle('open', failed);
      work.querySelector('.work-toggle').textContent = failed ? tr('v3.hide') : tr('v3.show');
      updateWork(work);
    }
  }

  // -- the plan, decisions and sources -------------------------------------------------------------

  /** The plan Cedar sent: editable while the run waits on it, a checklist once it is going. */
  function showPlan(view, data, awaiting) {
    const plan = cedarTrust.plan.parse(data);
    if (!plan) return;
    view.plan = plan;
    const el = cedarTrust.plan.element(plan, {
      awaiting: awaiting && !view.done,
      note: awaiting ? cedarTrust.plan.awaitingNote(plan) : tr('v3.plan.agreed'),
      onApprove: (steps) => approvePlan(view, steps),
    });
    if (view.planEl) view.planEl.replaceWith(el); else placeNote(view, el);
    view.planEl = el;
    view.stepsEl = null;
    if (awaiting && !view.done) setWorking(view, tr('v3.waitingPlan'));
  }

  /** The reader changed the steps before starting. */
  const planChanged = (view) => view.planEl.cedarPlan.setNote(tr('v3.plan.agreedChanges'));

  /** A card that waits for a decision (a suggested follow-up, a held call, what Cedar couldn't
   *  do) is a note in the conversation. */
  function placeDecision(view, card, heading, limitation) {
    const n = note('clarify', escapeHtml(heading), '');
    n.lastChild.append(card);
    placeNote(view, n);
    if (limitation) (view.limitNotes ||= []).push(n);
    view.stepsEl = null;
  }

  /** The sources rail beside a turn's cards: what it read, the files it saved, how it ran. */
  function renderSources(view) {
    const aside = view.sourcesEl;
    if (!aside) return;
    aside.textContent = '';
    const sources = view.sources = sourcesOf(view);
    if (sources.length) {
      aside.append(t('div', 'label', tr('v3.sourcesCount', { count: sources.length })));
      aside.append(cedarTrust.sources.element(sources, { onOpen: (s) => s.path && downloadFile(workspacePath(s.path)) }));
    }
    const files = [...new Set(view.tools.filter((tool) => FILE_TOOLS.has(tool.name) && tool.output !== undefined && !tool.is_error).map((tool) => workspacePath(tool.input?.path)).filter(Boolean))];
    if (files.length) {
      aside.append(t('div', 'label', tr('v3.workspace')));
      const list = h('div', 'files');
      for (const path of files) {
        const b = t('button', '', path);
        b.type = 'button';
        b.title = tr('v3.downloadPath', { path });
        b.addEventListener('click', () => downloadFile(path, b));
        list.append(b);
      }
      aside.append(list);
    }
    if (view.done) aside.append(t('div', 'sandbox', cedarTrust.sandbox.text(view.started, view.approvalsGranted)));
  }

  // -- the end of a turn ---------------------------------------------------------------------------

  /** A closing note goes where the turn's notes end. */
  const putClosing = (view, node, anchor) => (anchor?.isConnected ? els.notes.insertBefore(node, anchor) : els.notes.append(node));

  function closeTurn(view, event, anchor, put) {
    // What Cedar couldn't do, with its buttons, belongs after what it says about it: left where
    // the call was made, the answer written afterwards pushes it out of sight.
    for (const n of view.limitNotes || []) put(n);
    if (view.usage.input || view.usage.output) put(note('usage', '', cedarTrust.cost.text(view.usage)));
    anchor?.remove();
    view.planEl?.cedarPlan.finish(event.type === 'run.completed');
    settleWork(view);
    renderSources(view);
  }

  /** Once a watched turn is over, the stage rests on the top of its main result: the document it
   *  wrote, else its map, recipe, trip or stock card; a turn of searches and commands stays at the end. */
  function restOnResult(view) {
    els.notes.scrollTop = els.notes.scrollHeight;
    const main = view.tools.find((tool) => tool.card && cardKind(tool) === 'doc') || view.tools.find((tool) => tool.card && cardKind(tool) === 'widget');
    if (main) main.card.scrollIntoView({ block: 'start' });
  }

  function clearTurns() {
    els.notes.textContent = '';
    els.stageBody.textContent = '';
    els.stageEmpty.hidden = false;
    updateTabs();
  }

  // -- chrome --------------------------------------------------------------------------------------

  /** 128,397 → "128K" in the page's language. */
  function compactNumber(n) {
    try {
      return new Intl.NumberFormat(cedarI18n.locale, { notation: 'compact', maximumFractionDigits: n < 10_000 ? 1 : 0 }).format(n);
    } catch (e) {
      return String(n);
    }
  }

  /** The bar shows a short total ("128K tokens"); the split and the price are in its tooltip and
   *  in More › Run details. */
  function chromeChanged(c, total) {
    const used = c && (total.input || total.output);
    const full = used ? cedarTrust.cost.text(total) : '';
    els.cost.textContent = used ? tr('v3.tokensShort', { n: compactNumber(total.input + total.output) }) : '';
    els.cost.title = full;
    els.costFull.textContent = full;
    els.runDetails.hidden = !c || !(full || els.when.textContent);
  }

  // -- the two panes on a narrow screen ------------------------------------------------------------

  function setPane(pane) {
    els.app.dataset.pane = pane;
    if (pane === 'stage') state.unseen = 0;
    updateTabs();
  }

  function updateTabs() {
    for (const tab of els.tabs.querySelectorAll('.tab')) tab.setAttribute('aria-selected', String(tab.dataset.pane === els.app.dataset.pane));
    els.stageBadge.hidden = !state.unseen;
    els.stageBadge.textContent = state.unseen > 99 ? '99+' : String(state.unseen);
  }

  const narrowChanged = () => { if (!narrow.matches) { state.unseen = 0; updateTabs(); } };

  // -- the splitter --------------------------------------------------------------------------------

  const TALK_MIN = 300;
  const talkMax = () => Math.max(TALK_MIN, Math.min(720, window.innerWidth - 560));

  function setTalkWidth(px, save) {
    const w = Math.round(Math.min(talkMax(), Math.max(TALK_MIN, px)));
    document.documentElement.style.setProperty('--talk-w', `${w}px`);
    if (save) storage('set', SPLIT_KEY, String(w));
  }

  function bindSplitter() {
    const saved = Number(storage('get', SPLIT_KEY));
    if (saved) setTalkWidth(saved, false);
    let startX = 0;
    let startW = 0;
    els.splitter.addEventListener('pointerdown', (e) => {
      startX = e.clientX;
      startW = els.talk.getBoundingClientRect().width;
      els.splitter.setPointerCapture(e.pointerId);
      els.splitter.classList.add('active');
      els.app.classList.add('resizing');
    });
    els.splitter.addEventListener('pointermove', (e) => {
      if (!els.splitter.hasPointerCapture(e.pointerId)) return;
      setTalkWidth(startW + e.clientX - startX, false);
    });
    const end = (e) => {
      if (!els.splitter.hasPointerCapture(e.pointerId)) return;
      els.splitter.releasePointerCapture(e.pointerId);
      els.splitter.classList.remove('active');
      els.app.classList.remove('resizing');
      setTalkWidth(els.talk.getBoundingClientRect().width, true);
    };
    els.splitter.addEventListener('pointerup', end);
    els.splitter.addEventListener('pointercancel', end);
    els.splitter.addEventListener('dblclick', () => { document.documentElement.style.removeProperty('--talk-w'); storage('del', SPLIT_KEY); });
    els.splitter.addEventListener('keydown', (e) => {
      const step = e.key === 'ArrowLeft' ? -24 : e.key === 'ArrowRight' ? 24 : 0;
      if (!step) return;
      e.preventDefault();
      setTalkWidth(els.talk.getBoundingClientRect().width + step, true);
    });
    window.addEventListener('resize', () => {
      const w = parseInt(document.documentElement.style.getPropertyValue('--talk-w'), 10);
      if (w && w > talkMax()) setTalkWidth(w, false);
    });
  }

  // -- the conversations drawer --------------------------------------------------------------------

  const drawerOpen = () => !els.drawer.hidden;

  function openDrawer() {
    els.drawer.hidden = false;
    els.backdrop.hidden = false;
    els.chatsBtn.setAttribute('aria-expanded', 'true');
    renderRecents();
    (els.recents.querySelector('.conv.active') || els.recents.querySelector('.conv') || els.drawerClose).focus();
  }

  function closeDrawer() {
    if (els.drawer.hidden) return;
    els.drawer.hidden = true;
    els.backdrop.hidden = true;
    els.chatsBtn.setAttribute('aria-expanded', 'false');
  }

  /** The welcome screen's rail: the latest conversations, with when they were started. */
  function renderHomeRecents() {
    els.homeRecents.textContent = '';
    if (!state.conversations.length) { els.homeRecents.append(t('div', 'empty-list', tr('v3.homeEmpty'))); return; }
    for (const c of state.conversations.slice(0, 6)) {
      const a = h('a', 'home-conv');
      a.href = `#/c/${c.id}`;
      a.append(t('span', 'name', c.title || tr('v3.newChat')));
      const when = c.created_at ? new Date(c.created_at).toLocaleDateString(cedarI18n.locale, { weekday: 'short', day: 'numeric', month: 'short' }) : '';
      a.append(t('span', 'sub', [c.schedule_id ? tr('agents.badge') : null, when].filter(Boolean).join(' · ')));
      els.homeRecents.append(a);
    }
  }

  function bindLayout() {
    els.allChats.addEventListener('click', openDrawer);
    els.tabs.addEventListener('click', (e) => { const tab = e.target.closest('.tab'); if (tab) setPane(tab.dataset.pane); });
    bindSplitter();
    updateTabs();
  }

  return {
    narrow: '(max-width: 800px)',
    key: (name) => `v3.${name}`,
    scrollers: () => ({ notes: els.notes, cards: els.stage }),
    startTurn, placeNote, placeCard, focusCard, addStep, stepFilled, documentFilled: () => {},
    showPlan, planChanged, placeDecision, renderSources, putClosing, closeTurn, restOnResult,
    chromeChanged, conversationsChanged: renderHomeRecents, clearTurns, setPane, updateTabs,
    openDrawer, closeDrawer, drawerOpen, narrowChanged, bindLayout,
  };
})();

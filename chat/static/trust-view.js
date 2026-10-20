'use strict';

// The pieces that make a run legible to someone who did not write the prompt: the plan Cedar
// commits to before it starts (and that the user may edit), the questions it must ask before a
// risky tool call (an approval), its folded reasoning, the sources it cites, and what a turn
// cost. v3 ("Ledger") and v4 ("Dossier") both load it. Everything here degrades: a server that
// sends none of the new events (see chat/REDESIGN.md) simply shows none of these blocks.
//
// Events this module understands (all optional):
//   plan               { steps: [{id, title}], editable: bool }        -> plan.element
//   plan.updated       { steps }                                       -> plan.replace
//   assistant.reasoning{ turn, text, duration_ms? }                    -> reasoning.element
//   approval.required  { id, tool, input, reason, scope? }             -> approval.element
//   approval.resolved  { id, decision, by? }                           -> approval.settle
//   usage              { input_tokens, output_tokens, cost_usd? }      -> cost.text
//   run.started        { ..., sandbox?: { network, memory, cpus } }    -> sandbox.text
//   tool.call          { ..., step? }  (which plan step a call serves) -> plan.mark

const cedarTrust = (() => {
  const h = (tag, className, text) => {
    const el = document.createElement(tag);
    if (className) el.className = className;
    if (text !== undefined) el.textContent = text;
    return el;
  };
  const svg = (d) => `<svg viewBox="0 0 24 24" aria-hidden="true"><path d="${d}"/></svg>`;
  const CHEVRON = svg('M9 6l6 6-6 6');
  const CHECK = svg('M5 12.5l4.5 4.5L19 7');

  // -- the plan ------------------------------------------------------------------------------

  /** Normalise `{steps: [...]}` from the server; strings are accepted as titles. */
  function parsePlan(data) {
    const steps = Array.isArray(data?.steps) ? data.steps : [];
    const out = steps
      .map((s, i) => (typeof s === 'string' ? { id: String(i + 1), title: s } : s))
      .filter((s) => s && typeof s.title === 'string' && s.title.trim())
      .map((s, i) => ({ id: String(s.id ?? i + 1), title: s.title.trim() }));
    return out.length ? { steps: out, editable: data?.editable !== false } : null;
  }

  /**
   * The plan as a checklist. While `awaiting` (the run is paused on it) the steps are editable
   * text fields with Start and Edit buttons; `onApprove(steps)` sends them back. Once started
   * the rows show their state and how many calls served them.
   */
  function planElement(plan, { awaiting = false, onApprove, note } = {}) {
    const el = h('section', 'cedar-plan');
    el.dataset.state = awaiting ? 'awaiting' : 'running';
    const head = h('header', 'cedar-plan-head');
    head.append(h('span', 'cedar-plan-label', 'Plan'), h('span', 'cedar-plan-note', note || (awaiting ? 'Change anything, then start.' : '')));
    const count = h('span', 'cedar-plan-count', '');
    head.append(count);
    el.append(head);
    const list = h('ol', 'cedar-plan-steps');
    el.append(list);
    const rows = new Map();
    const row = (step) => {
      const li = h('li', 'cedar-plan-step');
      li.dataset.step = step.id;
      const mark = h('span', 'cedar-plan-mark');
      mark.innerHTML = CHECK;
      const title = h(awaiting ? 'input' : 'span', 'cedar-plan-title');
      if (awaiting) { title.value = step.title; title.setAttribute('aria-label', `Step ${step.id}`); }
      else title.textContent = step.title;
      const meta = h('span', 'cedar-plan-meta', '');
      li.append(mark, title, meta);
      if (awaiting) {
        const del = h('button', 'cedar-plan-remove', '');
        del.type = 'button';
        del.title = 'Remove this step';
        del.innerHTML = svg('M6 6l12 12M18 6L6 18');
        del.addEventListener('click', () => { li.remove(); rows.delete(step.id); });
        li.append(del);
      }
      rows.set(step.id, { li, meta, calls: 0 });
      return li;
    };
    for (const step of plan.steps) list.append(row(step));
    if (awaiting) {
      const acts = h('div', 'cedar-plan-actions');
      const add = h('button', 'cedar-plan-add', 'Add a step');
      add.type = 'button';
      add.addEventListener('click', () => {
        const id = String(Date.now());
        const li = row({ id, title: '' });
        list.append(li);
        li.querySelector('input').focus();
      });
      const start = h('button', 'cedar-plan-start', 'Looks good, start');
      start.type = 'button';
      start.addEventListener('click', async () => {
        const steps = [...list.querySelectorAll('.cedar-plan-step')]
          .map((li) => ({ id: li.dataset.step, title: li.querySelector('input').value.trim() }))
          .filter((s) => s.title);
        if (!steps.length) return;
        start.disabled = true;
        try { await onApprove?.(steps); } catch { start.disabled = false; }
      });
      acts.append(add, start);
      el.append(acts);
    }
    el.cedarPlan = {
      steps: plan.steps,
      /** A tool call began (or ended) for `stepId`. */
      mark(stepId, state) {
        const r = rows.get(String(stepId));
        if (!r) return;
        if (state === 'running') { r.calls++; r.li.dataset.state = 'running'; }
        else if (state === 'done') { r.li.dataset.state = 'done'; }
        else if (state === 'error') { r.li.dataset.state = 'error'; }
        r.meta.textContent = r.calls ? `${r.calls} call${r.calls === 1 ? '' : 's'}` : '';
        const done = [...rows.values()].filter((x) => x.li.dataset.state === 'done').length;
        count.textContent = `${done} / ${rows.size} done`;
      },
      /** The run is over: anything still running is done (or failed). */
      finish(ok) {
        for (const r of rows.values()) if (r.li.dataset.state !== 'done') r.li.dataset.state = ok ? 'done' : 'error';
        const done = [...rows.values()].filter((x) => x.li.dataset.state === 'done').length;
        count.textContent = `${done} / ${rows.size} done`;
      },
      setNote(text) { head.querySelector('.cedar-plan-note').textContent = text; },
    };
    return el;
  }

  // -- approvals -----------------------------------------------------------------------------

  const APPROVAL_KINDS = {
    download_file: { what: 'download from the web', scope: (i) => host(i.url) },
    bash: { what: 'run a command that changes the sandbox', scope: () => null },
    write_file: { what: 'overwrite a file', scope: () => null },
    fetch_url: { what: 'reach the web', scope: (i) => host(i.url) },
  };
  function host(url) { try { return new URL(url).hostname.replace(/^www\./, ''); } catch { return null; } }
  // What cedar's approval rules (`approval.required.rule`) are about.
  const RULE_WHAT = {
    email: 'send or change email',
    downloads: 'download from the web',
    installs: 'install packages',
    deletes: 'delete files',
    network: 'run code with internet access',
    community: 'run a community tool',
  };

  /**
   * "Cedar wants to <do a thing>. Allow once / Always for <scope> / Skip". `onDecide(decision)`
   * returns a promise; the card settles when it resolves. `spec`: {id, tool, input, reason, scope}.
   */
  function approvalElement(spec, { onDecide } = {}) {
    const kind = APPROVAL_KINDS[spec.tool] || { what: `use ${spec.tool}`, scope: () => null };
    const what = RULE_WHAT[spec.rule] || kind.what;
    // A cedar that names the rule also says what "always" would cover; none means ask every time.
    const scope = spec.rule ? spec.scope : spec.scope || kind.scope(spec.input || {});
    const el = h('section', 'cedar-approval');
    el.dataset.state = 'pending';
    const head = h('header', 'cedar-approval-head');
    head.append(h('span', 'cedar-approval-label', 'Needs your OK'), h('span', 'cedar-approval-what', `· ${what}`));
    el.append(head);
    const body = h('div', 'cedar-approval-body');
    body.textContent = spec.reason || describe(spec);
    el.append(body);
    const acts = h('div', 'cedar-approval-actions');
    const button = (label, decision, primary) => {
      const b = h('button', `cedar-approval-btn${primary ? ' primary' : ''}`, label);
      b.type = 'button';
      b.addEventListener('click', async () => {
        for (const x of acts.querySelectorAll('button')) x.disabled = true;
        try { await onDecide?.(decision); settle(el, decision); } catch { for (const x of acts.querySelectorAll('button')) x.disabled = false; }
      });
      return b;
    };
    acts.append(button('Allow once', 'allow', true));
    if (scope) acts.append(button(`Always for ${scope}`, 'allow_always'));
    acts.append(button('Skip', 'deny'));
    el.append(acts);
    return el;
  }

  function describe(spec) {
    const i = spec.input || {};
    switch (spec.tool) {
      case 'download_file': return `${i.url || 'a file'}${i.path ? ` → ${i.path}` : ''}`;
      case 'write_file': return `${i.path || 'a file'} already exists in the workspace.`;
      case 'bash': return i.command || '';
      default: return JSON.stringify(i);
    }
  }

  const DECISIONS = { allow: 'Allowed', allow_always: 'Allowed, always', deny: 'Skipped', timeout: 'Timed out' };
  /** Replace the buttons with what was decided and when. */
  function settle(el, decision, at) {
    el.dataset.state = decision === 'deny' || decision === 'timeout' ? 'denied' : 'allowed';
    const acts = el.querySelector('.cedar-approval-actions');
    acts.textContent = '';
    const time = new Date(at || Date.now()).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
    acts.append(h('span', 'cedar-approval-decision', `${DECISIONS[decision] || decision} · ${time}`));
  }

  // -- reasoning -----------------------------------------------------------------------------

  /** "Thought for 14 s" that opens to the text. Collapsed by default. */
  function reasoningElement(text, durationMs) {
    const el = h('details', 'cedar-reasoning');
    const sum = h('summary', '');
    sum.innerHTML = `${CHEVRON}<span></span>`;
    const secs = typeof durationMs === 'number' ? Math.max(1, Math.round(durationMs / 1000)) : null;
    sum.lastChild.textContent = secs ? `Thought for ${secs} s` : 'Thinking';
    const body = h('div', 'cedar-reasoning-body');
    body.textContent = text || '';
    el.append(sum, body);
    el.cedarReasoning = {
      append(more) { body.textContent += more; },
      done(ms) { const s = Math.max(1, Math.round((ms || 0) / 1000)); sum.lastChild.textContent = `Thought for ${s} s`; },
    };
    return el;
  }

  // -- sources and citations -----------------------------------------------------------------

  /**
   * The pages and files a turn read, numbered; `[n]` in Cedar's text points at one. A cedar that
   * numbers sources itself sends them with each result (`tool.sources`, the numbers the model
   * saw and cites by), and those are used as they are. Otherwise they are numbered here from the
   * finished tool calls: fetched and downloaded URLs, PDFs it read, then search results.
   * Returns [{n, url, title, host, kind, path?}].
   */
  function collectSources(tools, searchParser) {
    if (tools.some((t) => Array.isArray(t.sources))) return numberedByServer(tools);
    const out = [];
    const seen = new Set();
    const push = (s) => { const key = s.url || s.path; if (!key || seen.has(key)) return; seen.add(key); out.push({ n: out.length + 1, host: s.url ? host(s.url) : null, ...s }); };
    for (const t of tools) {
      if (t.output === undefined || t.is_error) continue;
      const i = t.input || {};
      if (t.name === 'fetch_url' && i.url) push({ url: i.url, title: titleOf(t.output) || i.url, kind: 'page' });
      else if (t.name === 'download_file' && i.url) push({ url: i.url, title: i.path?.split('/').pop() || i.url, kind: 'file', path: i.path });
    }
    // A document it read that it had just downloaded is the same source, not a second one.
    const downloaded = new Set(out.filter((s) => s.path).map((s) => s.path.replace(/^\/workspace\//, '')));
    for (const t of tools) {
      if (t.name !== 'read_document' || t.output === undefined || t.is_error) continue;
      const path = String(t.input?.path || '').replace(/^\/workspace\//, '');
      if (path && !downloaded.has(path)) push({ path, title: path.split('/').pop(), kind: 'document' });
    }
    for (const t of tools) {
      if (t.name !== 'web_search' || t.output === undefined || t.is_error) continue;
      for (const r of (searchParser ? searchParser(t.output) : [])) if (r.url) push({ url: r.url, title: r.title, kind: 'result', age: r.age });
    }
    return out;
  }

  const KIND_OF = { fetch_url: 'page', download_file: 'file', web_search: 'result' };
  function numberedByServer(tools) {
    const byN = new Map();
    for (const t of tools) {
      if (!Array.isArray(t.sources) || t.is_error) continue;
      for (const s of t.sources) {
        if (!s || typeof s.n !== 'number' || !s.url || byN.has(s.n)) continue;
        const path = t.name === 'download_file' ? t.input?.path : undefined;
        byN.set(s.n, { n: s.n, url: s.url, title: s.title || s.url, host: host(s.url), kind: KIND_OF[t.name] || 'page', ...(path ? { path } : {}) });
      }
    }
    return [...byN.values()].sort((a, b) => a.n - b.n);
  }

  /** The first heading-ish line of a fetched page, as fetch_url formats it. */
  function titleOf(output) {
    const first = String(output || '').split('\n').find((l) => l.trim());
    if (!first) return '';
    return first.replace(/^#+\s*/, '').replace(/^title:\s*/i, '').trim().slice(0, 120);
  }

  /** Turn "[n]" in rendered text into links to the numbered sources. Skips code and links. */
  function cite(root, sources, { onClick } = {}) {
    if (!sources?.length) return 0;
    const byN = new Map(sources.map((s) => [String(s.n), s]));
    const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT, {
      acceptNode: (node) => (node.parentElement.closest('code, pre, a, .cedar-cite') ? NodeFilter.FILTER_REJECT : NodeFilter.FILTER_ACCEPT),
    });
    const nodes = [];
    for (let n = walker.nextNode(); n; n = walker.nextNode()) if (/\[\d{1,2}\]/.test(n.data)) nodes.push(n);
    let count = 0;
    for (const node of nodes) {
      const frag = document.createDocumentFragment();
      let last = 0;
      for (const m of node.data.matchAll(/\[(\d{1,2})\]/g)) {
        const s = byN.get(m[1]);
        if (!s) continue;
        frag.append(node.data.slice(last, m.index));
        const a = h('a', 'cedar-cite', m[1]);
        a.href = s.url || '#';
        a.title = s.title;
        if (s.url) { a.target = '_blank'; a.rel = 'noopener noreferrer'; }
        if (onClick) a.addEventListener('click', (e) => { if (onClick(s, e) === false) e.preventDefault(); });
        const sup = h('sup', '', '');
        sup.append(a);
        frag.append(sup);
        last = m.index + m[0].length;
        count++;
      }
      if (!last) continue;
      frag.append(node.data.slice(last));
      node.replaceWith(frag);
    }
    return count;
  }

  /** The numbered list of sources. `onOpen(source)` for ones that live in the workspace. */
  function sourcesElement(sources, { onOpen } = {}) {
    const el = h('ol', 'cedar-sources');
    for (const s of sources) {
      const li = h('li', 'cedar-source');
      li.dataset.kind = s.kind;
      li.append(h('span', 'cedar-source-n', String(s.n)));
      const body = h('span', 'cedar-source-body');
      let title;
      if (s.url && /^https?:\/\//i.test(s.url)) {
        title = h('a', 'cedar-source-title', s.title);
        title.href = s.url; title.target = '_blank'; title.rel = 'noopener noreferrer';
      } else {
        title = h('button', 'cedar-source-title', s.title);
        title.type = 'button';
        title.addEventListener('click', () => onOpen?.(s));
      }
      body.append(title);
      const meta = [s.host, s.kind === 'file' ? 'saved to the workspace' : s.kind === 'document' ? 'workspace file' : s.age].filter(Boolean).join(' · ');
      if (meta) body.append(h('span', 'cedar-source-meta', meta));
      li.append(body);
      el.append(li);
    }
    return el;
  }

  // -- cost and the sandbox ------------------------------------------------------------------

  /** "6,412 in · 918 out · ≈ $0.004" (the cost only when the server sends one). */
  function costText(usage) {
    const parts = [`${(usage.input || 0).toLocaleString()} in`, `${(usage.output || 0).toLocaleString()} out`];
    if (typeof usage.cost_usd === 'number') parts.push(`≈ $${usage.cost_usd < 0.01 ? usage.cost_usd.toFixed(4) : usage.cost_usd.toFixed(2)}`);
    return parts.join(' · ');
  }

  /** "Ran in an isolated container · network off" from run.started, or the generic line. */
  function sandboxText(started, approvals) {
    const s = started?.sandbox;
    const parts = ['Ran in an isolated container'];
    if (s?.network) parts.push(s.network === 'none' ? 'network off' : `network: ${s.network}`);
    if (approvals) parts.push(`${approvals} approval${approvals === 1 ? '' : 's'} by you`);
    return parts.join(' · ');
  }

  return {
    plan: { parse: parsePlan, element: planElement },
    approval: { element: approvalElement, settle },
    reasoning: { element: reasoningElement },
    sources: { collect: collectSources, element: sourcesElement, cite },
    cost: { text: costText },
    sandbox: { text: sandboxText },
  };
})();

'use strict';

// Search over the conversations, shared by every UI: a search box in the conversations sidebar
// (drawn into the `[data-search]` mount) that finds chats and scheduled runs by their title and by
// what was said in them, the user's messages and cedar's answers alike
// (GET /api/conversations/search?q=). While something is typed, the results stand in for the
// sidebar's own lists, which carry `data-search-hides`; each result shows the passage that matched
// with the words marked, and opens its conversation. Ctrl+K (⌘K on a Mac) puts the cursor in the
// box from anywhere, Escape empties it, and the arrow keys walk the results.
//
// Each app hands over its own helpers through cedarSearch.init():
//
//   api(method, path, body) -> Promise<json>   the app's /api client (sends the access token)
//   onOpen()                                   optional: a result was opened (close a drawer)
//   reveal()                                   optional: bring the sidebar into view (open a drawer)

const cedarSearch = (() => {
  const DEBOUNCE_MS = 180;
  const GLASS = '<svg viewBox="0 0 24 24" aria-hidden="true"><circle cx="11" cy="11" r="6"/><path d="M20 20l-4.5-4.5"/></svg>';
  const CROSS = '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M7 7l10 10M17 7L7 17"/></svg>';
  const CLOCK = '<svg viewBox="0 0 24 24" aria-hidden="true"><circle cx="12" cy="12" r="9"/><path d="M12 7v5l3 2"/></svg>';

  let deps = null;
  let mount = null;
  let input = null;
  let clear = null;
  let results = null;
  let query = '';        // what is searched for, trimmed; '' when the box is empty
  let found = null;      // the conversations of the last answer, or null before the first one
  let failed = '';       // why the last search didn't work
  let request = 0;       // bumped on every search so a slow answer can't overwrite a newer one
  let timer = null;

  function el(tag, className, text) {
    const e = document.createElement(tag);
    if (className) e.className = className;
    if (text !== undefined) e.textContent = text;
    return e;
  }

  /** The words of the search, longest first, so "art" doesn't split the mark of "artist". */
  function words() {
    return [...new Set(query.toLowerCase().split(/\s+/).filter(Boolean))].sort((a, b) => b.length - a.length);
  }

  /** `text` into `parent`, with every word of the search in a <mark>. */
  function marked(parent, text) {
    const terms = words();
    const lower = text.toLowerCase();
    // Lowercasing changes the length of a few characters (İ); then the positions don't line up.
    if (!terms.length || lower.length !== text.length) {
      parent.append(text);
      return;
    }
    let at = 0;
    while (at < text.length) {
      let next = -1;
      let len = 0;
      for (const t of terms) {
        const i = lower.indexOf(t, at);
        if (i !== -1 && (next === -1 || i < next)) { next = i; len = t.length; }
      }
      if (next === -1) break;
      if (next > at) parent.append(text.slice(at, next));
      parent.append(el('mark', '', text.slice(next, next + len)));
      at = next + len;
    }
    if (at < text.length) parent.append(text.slice(at));
  }

  function openId() {
    const m = /^#\/c\/([^/?]+)/.exec(location.hash);
    return m ? m[1] : '';
  }

  function render() {
    const searching = Boolean(query);
    mount.parentElement.classList.toggle('cedar-searching', searching);
    mount.classList.toggle('is-active', searching);
    clear.hidden = !input.value;
    results.hidden = !searching;
    results.textContent = '';
    if (!searching) return;
    if (failed) {
      results.append(el('p', 'cedar-search-note', failed));
    } else if (!found) {
      results.append(el('p', 'cedar-search-note', tr('search.searching')));
    } else if (!found.length) {
      results.append(el('p', 'cedar-search-note', tr('search.none', { query })));
    }
    const open = openId();
    for (const c of found || []) {
      const title = c.title || tr('star.newChat');
      const link = el('a', 'cedar-search-hit');
      link.href = `#/c/${c.id}`;
      link.classList.toggle('active', c.id === open);
      link.classList.toggle('is-unread', Boolean(c.unread));
      link.title = c.schedule_title ? tr('star.scheduledBy', { title, schedule: c.schedule_title }) : title;
      const head = el('span', 'cedar-search-hit-head');
      // A scheduled run gets the clock the schedule list uses, so the two kinds are told apart.
      if (c.schedule_id) head.insertAdjacentHTML('beforeend', CLOCK);
      marked(head.appendChild(el('span', 'cedar-search-hit-title')), title);
      const when = new Date(c.updated_at);
      if (!Number.isNaN(when.getTime())) {
        head.append(el('span', 'cedar-search-hit-when', when.toLocaleDateString(cedarI18n.locale || undefined, { month: 'short', day: 'numeric', year: when.getFullYear() === new Date().getFullYear() ? undefined : 'numeric' })));
      }
      link.append(head);
      if (c.snippet) marked(link.appendChild(el('span', 'cedar-search-hit-text')), c.snippet);
      link.addEventListener('click', () => deps.onOpen?.());
      results.append(link);
    }
  }

  async function search() {
    const mine = ++request;
    const q = query;
    if (!q) return;
    try {
      const res = await deps.api('GET', `/conversations/search?q=${encodeURIComponent(q)}`);
      if (mine !== request) return;
      found = res.conversations || [];
      failed = '';
    } catch (e) {
      if (mine !== request) return;
      found = null;
      failed = e.message || tr('search.failed');
    }
    render();
  }

  /** The box changed: search for what it holds now, a moment after the last keystroke. */
  function changed() {
    const q = input.value.trim().replace(/\s+/g, ' ');
    clear.hidden = !input.value;
    if (q === query) return;
    query = q;
    clearTimeout(timer);
    request++; // an answer still on its way is for the old text
    failed = '';
    if (!q) {
      found = null;
      render();
      return;
    }
    // The old results stay up while the new ones load, unless there were none to show.
    if (!found) render();
    timer = setTimeout(search, DEBOUNCE_MS);
  }

  function reset() {
    input.value = '';
    changed();
  }

  function hits() {
    return [...results.querySelectorAll('.cedar-search-hit')];
  }

  function onInputKey(e) {
    if (e.key === 'Escape' && input.value) {
      e.preventDefault();
      e.stopPropagation(); // the first Escape empties the box; the next one is the page's
      reset();
    } else if (e.key === 'ArrowDown') {
      const first = hits()[0];
      if (first) { e.preventDefault(); first.focus(); }
    } else if (e.key === 'Enter') {
      const first = hits()[0];
      if (first) { e.preventDefault(); first.click(); }
    }
  }

  function onResultsKey(e) {
    const all = hits();
    const i = all.indexOf(document.activeElement);
    if (i === -1) return;
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      all[Math.min(i + 1, all.length - 1)].focus();
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      if (i === 0) input.focus(); else all[i - 1].focus();
    } else if (e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      input.focus();
    }
  }

  /** Put the cursor in the box, bringing the sidebar into view first. */
  function focus() {
    if (!input) return;
    deps.reveal?.();
    input.focus();
    input.select();
  }

  function bind() {
    mount = document.querySelector('[data-search]');
    if (!mount) return;
    mount.classList.add('cedar-search');
    const box = el('label', 'cedar-search-box');
    box.insertAdjacentHTML('beforeend', GLASS);
    input = el('input', 'cedar-search-input');
    input.type = 'search';
    input.autocomplete = 'off';
    input.spellcheck = false;
    input.enterKeyHint = 'search';
    input.maxLength = 200;
    input.placeholder = tr('search.placeholder');
    input.setAttribute('aria-label', tr('search.placeholder'));
    const mac = /Mac|iPhone|iPad/.test(navigator.platform || '');
    input.title = tr('search.hint', { keys: mac ? '⌘K' : 'Ctrl+K' });
    clear = el('button', 'cedar-search-clear');
    clear.type = 'button';
    clear.hidden = true;
    clear.setAttribute('aria-label', tr('search.clear'));
    clear.title = tr('search.clear');
    clear.insertAdjacentHTML('beforeend', CROSS);
    box.append(input, clear);
    results = el('nav', 'cedar-search-results');
    results.hidden = true;
    results.setAttribute('aria-label', tr('search.results'));
    results.setAttribute('aria-live', 'polite');
    mount.append(box, results);

    input.addEventListener('input', changed);
    input.addEventListener('keydown', onInputKey);
    clear.addEventListener('click', () => { reset(); input.focus(); });
    results.addEventListener('keydown', onResultsKey);
    document.addEventListener('keydown', (e) => {
      if ((e.metaKey || e.ctrlKey) && !e.shiftKey && !e.altKey && e.key.toLowerCase() === 'k') {
        e.preventDefault();
        focus();
      }
    });
    // The open conversation is marked in the results; a renamed or deleted one is found afresh.
    window.addEventListener('hashchange', () => { if (query) render(); });
    document.addEventListener('visibilitychange', () => { if (query && document.visibilityState === 'visible') search(); });
  }

  function init(helpers) {
    deps = helpers;
    if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', bind);
    else bind();
  }

  /** Search again for what the box holds: a conversation was renamed, deleted or answered. */
  function refresh() {
    if (query) search();
  }

  return { init, focus, refresh, reset };
})();

'use strict';

// Recipe display for cedar's `show_recipe` tool, shared by both UIs (/v1 and /v2). The tool's result
// is a JSON object, {"title", "description"?, "servings"?, "prep_time"?, "cook_time"?, "total_time"?,
// "ingredients": [{"quantity"?, "item"}], "steps": [{"title"?, "text", "duration"?}], "tips"?: [text],
// "image"?: {"url", "page_url", "title", "source"}}, and this turns it into a card: the photo of the
// dish, the times, the ingredients as a checklist, and the steps as a timeline that appears one
// step after another and can be worked through:
//
//   cedarRecipe.parse(output) -> spec | null   the validated result, or null if it isn't a recipe
//   cedarRecipe.element(spec) -> HTMLElement   the card
//
// It needs nothing from the app. Everything in the result came from the model, so text is only ever
// set as text, and the photo and its page are only used if they are https URLs.

const cedarRecipe = (() => {
  const MAX_INGREDIENTS = 60;
  const MAX_STEPS = 40;
  const MAX_TIPS = 10;
  const MAX_TEXT = 200;
  const MAX_LONG = 2000;
  /** How far apart the steps appear when the card is first shown; recipe-view.css uses it too. */
  const STEP_STAGGER_MS = 110;

  function el(tag, className, text) {
    const e = document.createElement(tag);
    if (className) e.className = className;
    if (text !== undefined) e.textContent = text;
    return e;
  }

  const str = (v, max = MAX_TEXT) => (typeof v === 'string' && v.trim() ? v.trim().slice(0, max) : '');
  const https = (v) => (typeof v === 'string' && /^https:\/\/[^\s]+$/.test(v) ? v : '');

  /** The recipe in a `show_recipe` result, or null if `output` isn't one with ingredients and steps. */
  function parse(output) {
    let data;
    try { data = JSON.parse(output); } catch { return null; }
    if (!data || typeof data !== 'object' || !Array.isArray(data.ingredients) || !Array.isArray(data.steps)) return null;
    const title = str(data.title);
    if (!title) return null;
    const ingredients = data.ingredients.slice(0, MAX_INGREDIENTS)
      .map((i) => ({ quantity: str(i?.quantity), item: str(i?.item) }))
      .filter((i) => i.item);
    const steps = data.steps.slice(0, MAX_STEPS)
      .map((s) => ({ title: str(s?.title), text: str(s?.text, MAX_LONG), duration: str(s?.duration) }))
      .filter((s) => s.text);
    if (!ingredients.length || !steps.length) return null;
    const tips = Array.isArray(data.tips) ? data.tips.map((t) => str(t, MAX_LONG)).filter(Boolean).slice(0, MAX_TIPS) : [];
    const img = data.image;
    const image = img && https(img.url)
      ? { url: img.url, pageUrl: https(img.page_url), title: str(img.title), source: str(img.source) || 'Wikimedia' }
      : null;
    const meta = [['Prep', data.prep_time], ['Cook', data.cook_time], ['Total', data.total_time], ['Serves', data.servings]]
      .map(([label, v]) => [label, str(v)])
      .filter(([, v]) => v);
    return { title, description: str(data.description, MAX_LONG), meta, ingredients, steps, tips, image };
  }

  const CHECK = '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M5 12.5l4.5 4.5L19 7.5"/></svg>';
  const CLOCK = '<svg viewBox="0 0 24 24" aria-hidden="true"><circle cx="12" cy="12" r="8.5"/><path d="M12 7.5V12l3 2"/></svg>';

  /** The photo with the title over it, or just the title when there is no photo or it fails to load. */
  function header(spec) {
    const head = el('div', 'cedar-recipe-head');
    const title = el('div', 'cedar-recipe-title', spec.title);
    title.setAttribute('role', 'heading');
    title.setAttribute('aria-level', '3');
    if (!spec.image) {
      head.append(title);
      return head;
    }
    head.classList.add('has-photo');
    const img = el('img', 'cedar-recipe-photo');
    img.alt = spec.image.title ? `Photo: ${spec.image.title}` : `Photo of ${spec.title}`;
    img.decoding = 'async';
    img.referrerPolicy = 'no-referrer';
    img.addEventListener('load', () => head.classList.add('loaded'));
    img.addEventListener('error', () => {
      head.classList.remove('has-photo', 'loaded');
      img.remove();
      credit.remove();
    });
    img.src = spec.image.url;
    const credit = el(spec.image.pageUrl ? 'a' : 'span', 'cedar-recipe-credit');
    credit.textContent = `Photo: ${spec.image.source}`;
    if (spec.image.pageUrl) {
      credit.href = spec.image.pageUrl;
      credit.target = '_blank';
      credit.rel = 'noopener noreferrer';
      credit.title = spec.image.title ? `${spec.image.title} on ${spec.image.source}` : spec.image.source;
    }
    head.append(img, title, credit);
    return head;
  }

  function ingredientList(spec) {
    const section = el('section', 'cedar-recipe-section');
    const heading = el('div', 'cedar-recipe-label', 'Ingredients');
    const list = el('ul', 'cedar-recipe-ingredients');
    for (const ing of spec.ingredients) {
      const li = el('li');
      const label = el('label');
      const box = el('input');
      box.type = 'checkbox';
      box.addEventListener('change', () => li.classList.toggle('got', box.checked));
      label.append(box);
      if (ing.quantity) label.append(el('span', 'cedar-recipe-qty', ing.quantity));
      label.append(el('span', 'cedar-recipe-item', ing.item));
      li.append(label);
      list.append(li);
    }
    section.append(heading, list);
    return section;
  }

  /**
   * The steps as a timeline: a numbered node per step on a line that fills in as steps are done.
   * The first step not done yet is the current one. A node marks its step done (or not again), and
   * the button under the timeline finishes the current step and moves on.
   */
  function stepTimeline(spec) {
    const section = el('section', 'cedar-recipe-section');
    const top = el('div', 'cedar-recipe-label');
    const progress = el('span', 'cedar-recipe-progress');
    progress.setAttribute('aria-live', 'polite');
    top.append(el('span', '', 'Steps'), progress);
    const list = el('ol', 'cedar-recipe-steps');
    const done = spec.steps.map(() => false);
    const items = spec.steps.map((step, i) => {
      const li = el('li', 'cedar-step');
      li.style.setProperty('--i', String(i));
      const node = el('button', 'cedar-step-node');
      node.type = 'button';
      node.innerHTML = `<span class="n">${i + 1}</span>${CHECK}`;
      node.addEventListener('click', () => { done[i] = !done[i]; update(); });
      const body = el('div', 'cedar-step-body');
      if (step.title || step.duration) {
        const line = el('div', 'cedar-step-head');
        if (step.title) line.append(el('span', 'cedar-step-title', step.title));
        if (step.duration) {
          const time = el('span', 'cedar-step-time');
          time.innerHTML = CLOCK;
          time.append(step.duration);
          line.append(time);
        }
        body.append(line);
      }
      body.append(el('p', 'cedar-step-text', step.text));
      li.append(node, body);
      list.append(li);
      return { li, node };
    });

    const next = el('button', 'cedar-recipe-next');
    next.type = 'button';
    next.addEventListener('click', () => {
      const current = done.indexOf(false);
      if (current >= 0) done[current] = true;
      else done.fill(false);
      update();
      const after = done.indexOf(false);
      if (after >= 0) items[after].node.focus({ preventScroll: true });
    });

    function update() {
      const current = done.indexOf(false);
      items.forEach(({ li, node }, i) => {
        li.classList.toggle('done', done[i]);
        li.classList.toggle('current', i === current);
        node.setAttribute('aria-pressed', String(done[i]));
        node.setAttribute('aria-label', `Step ${i + 1}: ${done[i] ? 'done, mark as not done' : 'mark as done'}`);
        if (i === current) li.setAttribute('aria-current', 'step');
        else li.removeAttribute('aria-current');
      });
      const count = done.filter(Boolean).length;
      progress.textContent = count ? `${count} of ${done.length} done` : `${done.length} step${done.length === 1 ? '' : 's'}`;
      next.textContent = current < 0 ? 'All done. Start over' : current === done.length - 1 ? 'Finish' : `Done with step ${current + 1}`;
      next.classList.toggle('finished', current < 0);
    }
    update();
    section.append(top, list, next);
    return section;
  }

  /** The card for `spec`. Its steps appear one after another the first time it is on the page. */
  function element(spec) {
    const card = el('div', 'cedar-recipe');
    card.setAttribute('role', 'group');
    card.setAttribute('aria-label', `Recipe: ${spec.title}`);
    card.append(header(spec));

    if (spec.meta.length) {
      const meta = el('div', 'cedar-recipe-meta');
      for (const [label, value] of spec.meta) {
        const chip = el('span', 'cedar-recipe-chip');
        chip.append(el('span', 'k', label), el('span', 'v', value));
        meta.append(chip);
      }
      card.append(meta);
    }
    if (spec.description) card.append(el('p', 'cedar-recipe-desc', spec.description));

    const body = el('div', 'cedar-recipe-body');
    body.append(ingredientList(spec), stepTimeline(spec));
    card.append(body);

    if (spec.tips.length) {
      const tips = el('section', 'cedar-recipe-tips');
      tips.append(el('div', 'cedar-recipe-label', spec.tips.length === 1 ? 'Tip' : 'Tips'));
      const list = el('ul');
      for (const t of spec.tips) list.append(el('li', '', t));
      tips.append(list);
      card.append(tips);
    }

    // The reveal runs once, when the card is first visible, so a card scrolled to later still plays
    // it, and one moved around the page (the desk redraws) doesn't play it again.
    card.classList.add('reveal');
    card.style.setProperty('--steps', String(spec.steps.length));
    const seen = new IntersectionObserver((entries) => {
      if (!entries.some((e) => e.isIntersecting)) return;
      seen.disconnect();
      card.classList.add('play');
      const total = spec.steps.length * STEP_STAGGER_MS + 900;
      setTimeout(() => card.classList.remove('reveal', 'play'), total);
    });
    seen.observe(card);
    return card;
  }

  return { parse, element };
})();

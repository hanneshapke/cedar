'use strict';

// Product display for cedar's `search_products` tool, shared by every UI. The tool's result is a
// JSON object, {"query", "store", "currency", "total", "as_of", "sort", "filters", "options":
// [{"sku", "name", "brand"?, "model"?, "color"?, "price", "regular_price"?, "savings_pct"?, "url"?,
// "images": [url], "description"?, "features": [text], "specs": [{"name", "value"}], "category"?,
// "availability": {"online", "in_store", "free_shipping"}, "rank"?, "reviews": {"average", "count",
// "adjusted", "verdict", "confidence", "summary"}, "comparable": [sku]}], "others": [{... "image"?}],
// "picks": {"best_rated"?, "best_value"?, "most_popular"?}, "disclaimer"}, and this turns it into a
// card:
//
//   - a tile per option (its photo, badges for the picks, price and savings, stars and the number
//     of reviews, the verdict), which picks the product shown below
//   - the selected product: a photo gallery, the price, availability, a link to the store page,
//     the reading of its reviews (stars, the adjusted rating on a bar, the verdict and how much to
//     trust it), the description, key features, and specs
//   - the comparable options for it, a strip of small tiles that can be viewed in turn
//
//   cedarProducts.parse(output) -> spec | null   the validated result, or null if it isn't one
//   cedarProducts.element(spec) -> HTMLElement   the card
//
// It needs nothing from the app. Everything in the result came from the store or the model, so
// text is only ever set as text, and links and pictures are only used if they are https URLs.

const cedarProducts = (() => {
  const MAX_OPTIONS = 8;
  const MAX_OTHERS = 40;
  const MAX_IMAGES = 6;
  const MAX_FEATURES = 6;
  const MAX_SPECS = 8;
  const MAX_COMPARABLE = 4;
  const MAX_TEXT = 200;
  const MAX_LONG = 600;
  const VERDICTS = { excellent: 'Excellent', good: 'Good', mixed: 'Mixed', poor: 'Poor', few: 'Few reviews', none: 'No reviews' };
  const CONFIDENCE = { high: 'high confidence', medium: 'medium confidence', low: 'low confidence' };
  const PICKS = [['best_rated', 'Best rated'], ['best_value', 'Best value'], ['most_popular', 'Most popular']];
  const SORTS = { best_selling: 'best selling first', rating: 'highest rated first', price_low: 'cheapest first', price_high: 'priciest first' };
  /** How far apart the tiles appear when the card is first shown; product-view.css uses it too. */
  const TILE_STAGGER_MS = 90;

  function el(tag, className, text) {
    const e = document.createElement(tag);
    if (className) e.className = className;
    if (text !== undefined) e.textContent = text;
    return e;
  }

  const str = (v, max = MAX_TEXT) => (typeof v === 'string' && v.trim() ? v.trim().slice(0, max) : '');
  const num = (v) => (typeof v === 'number' && Number.isFinite(v) ? v : null);
  const https = (v) => (typeof v === 'string' && /^https:\/\/[^\s"'<>]+$/.test(v) ? v : '');

  function parseReviews(r) {
    const o = r && typeof r === 'object' ? r : {};
    const count = Math.max(0, Math.round(num(o.count) ?? 0));
    const average = num(o.average);
    return {
      average: average !== null ? Math.max(0, Math.min(5, average)) : null,
      count,
      adjusted: num(o.adjusted) !== null ? Math.max(0, Math.min(5, num(o.adjusted))) : null,
      verdict: VERDICTS[o.verdict] ? o.verdict : count ? 'few' : 'none',
      confidence: CONFIDENCE[o.confidence] ? o.confidence : 'low',
      summary: str(o.summary, MAX_LONG),
    };
  }

  /** One product, from a shown option (with pictures, features, specs) or one of the others. */
  function parseProduct(p) {
    if (!p || typeof p !== 'object') return null;
    const sku = num(p.sku) !== null ? String(Math.round(p.sku)) : str(p.sku, 40);
    const name = str(p.name);
    const price = num(p.price);
    if (!sku || !name || price === null || price < 0) return null;
    const images = (Array.isArray(p.images) ? p.images : [p.image]).map(https).filter(Boolean).slice(0, MAX_IMAGES);
    const regular = num(p.regular_price);
    const a = p.availability && typeof p.availability === 'object' ? p.availability : {};
    return {
      sku,
      name,
      brand: str(p.brand, 80),
      model: str(p.model, 80),
      color: str(p.color, 60),
      condition: str(p.condition, 40),
      price,
      regularPrice: regular !== null && regular > price ? regular : null,
      savingsPct: num(p.savings_pct),
      url: https(p.url),
      images,
      description: str(p.description, MAX_LONG),
      features: (Array.isArray(p.features) ? p.features : []).map((f) => str(f)).filter(Boolean).slice(0, MAX_FEATURES),
      specs: (Array.isArray(p.specs) ? p.specs : []).map((s) => [str(s?.name, 80), str(s?.value)]).filter(([n, v]) => n && v).slice(0, MAX_SPECS),
      category: str(p.category, 80),
      availability: { online: a.online === true, inStore: a.in_store === true, freeShipping: a.free_shipping === true },
      rank: num(p.rank),
      reviews: parseReviews(p.reviews),
      comparable: (Array.isArray(p.comparable) ? p.comparable : []).map((s) => (num(s) !== null ? String(Math.round(s)) : str(s, 40))).filter(Boolean).slice(0, MAX_COMPARABLE),
    };
  }

  /** The products in a `search_products` result, or null if `output` isn't one with an option. */
  function parse(output) {
    let data;
    try { data = JSON.parse(output); } catch { return null; }
    if (!data || typeof data !== 'object' || !Array.isArray(data.options)) return null;
    const options = data.options.slice(0, MAX_OPTIONS).map(parseProduct).filter(Boolean);
    if (!options.length) return null;
    const known = new Set(options.map((o) => o.sku));
    const others = (Array.isArray(data.others) ? data.others : []).slice(0, MAX_OTHERS).map(parseProduct)
      .filter((p) => p && !known.has(p.sku));
    const bySku = new Map([...options, ...others].map((p) => [p.sku, p]));
    for (const p of options) p.comparable = p.comparable.filter((s) => s !== p.sku && bySku.has(s));
    const picks = data.picks && typeof data.picks === 'object' ? data.picks : {};
    const badges = new Map();
    for (const [key, label] of PICKS) {
      const sku = num(picks[key]) !== null ? String(Math.round(picks[key])) : str(picks[key], 40);
      if (known.has(sku)) badges.set(sku, [...(badges.get(sku) || []), label]);
    }
    return {
      query: str(data.query, 160) || 'Products',
      store: str(data.store, 60) || 'the store',
      currency: str(data.currency, 8) || 'USD',
      total: Math.max(options.length + others.length, Math.round(num(data.total) ?? 0)),
      asOf: Date.parse(str(data.as_of, 40)),
      sort: SORTS[data.sort] || '',
      options,
      others,
      bySku,
      badges,
      disclaimer: str(data.disclaimer, MAX_LONG),
    };
  }

  // -- formatting ----------------------------------------------------------------------------

  const SYMBOLS = { USD: '$', EUR: '€', GBP: '£', JPY: '¥', CAD: 'C$', AUD: 'A$' };

  function money(x, currency) {
    if (x === null) return '–';
    const n = x.toLocaleString(undefined, { minimumFractionDigits: 2, maximumFractionDigits: 2 });
    const sym = SYMBOLS[currency];
    return sym ? `${sym}${n}` : `${n} ${currency}`;
  }

  const count = (n) => `${n.toLocaleString()} review${n === 1 ? '' : 's'}`;

  const ICONS = {
    bag: '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M5 8h14l-1 12H6zM9 8V6a3 3 0 0 1 6 0v2"/></svg>',
    photo: '<svg viewBox="0 0 24 24" aria-hidden="true"><rect x="3" y="5" width="18" height="14" rx="2"/><circle cx="9" cy="10" r="1.5"/><path d="M21 16l-5-5-9 8"/></svg>',
    out: '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M14 5h5v5M19 5l-8 8M19 14v5H5V5h5"/></svg>',
    back: '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M15 6l-6 6 6 6"/></svg>',
    star: '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M12 3.5l2.6 5.6 6.1.7-4.5 4.2 1.2 6-5.4-3-5.4 3 1.2-6L3.3 9.8l6.1-.7z"/></svg>',
    check: '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M5 12.5l4.5 4.5L19 7.5"/></svg>',
  };

  /** Five stars, filled to `value` out of 5. */
  function stars(value, small) {
    const wrap = el('span', `cedar-products-stars${small ? ' small' : ''}`);
    wrap.setAttribute('role', 'img');
    wrap.setAttribute('aria-label', value === null ? 'Not rated' : `${value.toFixed(1)} out of 5 stars`);
    const base = el('span', 'base');
    base.innerHTML = ICONS.star.repeat(5);
    const fill = el('span', 'fill');
    fill.innerHTML = ICONS.star.repeat(5);
    fill.style.width = `${((value ?? 0) / 5) * 100}%`;
    wrap.append(base, fill);
    return wrap;
  }

  function verdictPill(reviews, small) {
    const pill = el('span', `cedar-products-verdict ${reviews.verdict}${small ? ' small' : ''}`, VERDICTS[reviews.verdict]);
    pill.title = reviews.summary;
    return pill;
  }

  /** An image that drops out, leaving a placeholder, if it can't be loaded. */
  function picture(url, alt, className) {
    const box = el('span', `cedar-products-photo ${className || ''}`);
    if (!url) {
      box.classList.add('empty');
      box.innerHTML = ICONS.photo;
      return box;
    }
    const img = el('img');
    img.alt = alt;
    img.decoding = 'async';
    img.loading = 'lazy';
    img.referrerPolicy = 'no-referrer';
    img.addEventListener('load', () => box.classList.add('loaded'));
    img.addEventListener('error', () => { img.remove(); box.classList.add('empty'); box.innerHTML = ICONS.photo; });
    img.src = url;
    box.append(img);
    return box;
  }

  function priceLine(p, currency, big) {
    const row = el('div', `cedar-products-price${big ? ' big' : ''}`);
    row.append(el('span', 'now', money(p.price, currency)));
    if (p.regularPrice !== null) {
      const was = el('s', 'was', money(p.regularPrice, currency));
      was.setAttribute('aria-label', `was ${money(p.regularPrice, currency)}`);
      row.append(was);
      const pct = p.savingsPct ?? Math.round((1 - p.price / p.regularPrice) * 100);
      if (pct >= 1) row.append(el('span', 'save', `Save ${Math.round(pct)}%`));
    }
    return row;
  }

  // -- the tiles -------------------------------------------------------------------------------

  function tile(p, spec, i) {
    const b = el('button', 'cedar-products-tile');
    b.type = 'button';
    b.setAttribute('role', 'tab');
    b.dataset.sku = p.sku;
    b.style.setProperty('--i', String(i));
    b.append(picture(p.images[0], '', 'ph-tile'));
    const badges = spec.badges.get(p.sku) || [];
    if (badges.length) {
      const row = el('span', 'badges');
      for (const label of badges) row.append(el('span', 'badge', label));
      b.append(row);
    }
    b.append(el('span', 'name', p.name));
    if (p.brand) b.append(el('span', 'brand', p.brand));
    b.append(priceLine(p, spec.currency, false));
    const r = p.reviews;
    const rating = el('span', 'rating');
    rating.append(stars(r.average, true));
    if (r.average !== null) rating.append(el('span', 'avg', r.average.toFixed(1)));
    rating.append(el('span', 'n', r.count ? `(${r.count.toLocaleString()})` : 'no reviews'));
    b.append(rating, verdictPill(r, true));
    b.setAttribute('aria-label', `${p.name}, ${money(p.price, spec.currency)}, ${r.average !== null ? `${r.average.toFixed(1)} stars from ${count(r.count)}` : 'no reviews'}${badges.length ? `, ${badges.join(', ')}` : ''}`);
    return b;
  }

  // -- the detail panel ------------------------------------------------------------------------

  function gallery(p) {
    const g = el('div', 'cedar-products-gallery');
    const main = el('div', 'main');
    const thumbs = el('div', 'thumbs');
    thumbs.setAttribute('role', 'group');
    thumbs.setAttribute('aria-label', `Photos of ${p.name}`);
    const buttons = [];
    let current = 0;
    function show(i) {
      current = (i + p.images.length) % p.images.length;
      main.replaceChildren(picture(p.images[current], `${p.name}, photo ${current + 1} of ${p.images.length}`, 'ph-main'));
      buttons.forEach((b, k) => b.setAttribute('aria-pressed', String(k === current)));
    }
    if (!p.images.length) {
      main.append(picture('', '', 'ph-main'));
      g.append(main);
      return g;
    }
    p.images.forEach((url, i) => {
      const b = el('button', 'thumb');
      b.type = 'button';
      b.setAttribute('aria-label', `Photo ${i + 1}`);
      b.append(picture(url, '', 'ph-thumb'));
      b.addEventListener('click', () => show(i));
      b.addEventListener('keydown', (e) => {
        if (e.key === 'ArrowRight' || e.key === 'ArrowLeft') {
          e.preventDefault();
          show(current + (e.key === 'ArrowRight' ? 1 : -1));
          buttons[current].focus();
        }
      });
      buttons.push(b);
      thumbs.append(b);
    });
    show(0);
    g.append(main);
    if (p.images.length > 1) g.append(thumbs);
    return g;
  }

  function reviewsBlock(p) {
    const r = p.reviews;
    const box = el('section', 'cedar-products-reviews');
    box.append(el('div', 'cedar-products-label', 'Reviews'));
    const head = el('div', 'head');
    head.append(stars(r.average, false));
    if (r.average !== null) {
      head.append(el('span', 'avg', r.average.toFixed(1)));
      head.append(el('span', 'n', `from ${count(r.count)}`));
    } else {
      head.append(el('span', 'n', 'No customer reviews yet'));
    }
    box.append(head);
    if (r.adjusted !== null && r.count > 0) {
      const bar = el('div', 'bar');
      bar.setAttribute('role', 'img');
      bar.setAttribute('aria-label', `Adjusted rating ${r.adjusted.toFixed(1)} out of 5`);
      const track = el('div', 'track');
      const fillEl = el('div', `fill ${r.verdict}`);
      fillEl.style.width = `${(r.adjusted / 5) * 100}%`;
      track.append(fillEl);
      for (const v of [1, 2, 3, 4]) {
        const tick = el('span', 'tick');
        tick.style.left = `${(v / 5) * 100}%`;
        track.append(tick);
      }
      const legend = el('div', 'legend');
      legend.append(el('span', 'k', 'Adjusted for the number of reviews'), el('strong', 'v', `${r.adjusted.toFixed(1)} / 5`));
      bar.append(legend, track);
      box.append(bar);
    }
    const line = el('div', 'verdict-line');
    line.append(verdictPill(r, false));
    if (r.count > 0) line.append(el('span', 'conf', CONFIDENCE[r.confidence]));
    box.append(line);
    if (r.summary) box.append(el('p', 'summary', r.summary));
    return box;
  }

  function availabilityChips(p) {
    const row = el('div', 'cedar-products-avail');
    const chip = (text, ok) => {
      const c = el('span', `chip${ok ? ' ok' : ' no'}`);
      if (ok) c.innerHTML = ICONS.check;
      c.append(text);
      row.append(c);
    };
    chip(p.availability.online ? 'Available online' : 'Not available online', p.availability.online);
    if (p.availability.inStore) chip('In store', true);
    if (p.availability.freeShipping) chip('Free shipping', true);
    if (p.condition && !/^new$/i.test(p.condition)) chip(p.condition, false);
    return row;
  }

  function comparableStrip(p, spec, state, select) {
    const items = p.comparable.map((s) => spec.bySku.get(s)).filter(Boolean);
    if (!items.length) return null;
    const section = el('section', 'cedar-products-comparable');
    section.append(el('div', 'cedar-products-label', 'Comparable options'));
    const strip = el('div', 'strip');
    for (const c of items) {
      const b = el('button', 'mini');
      b.type = 'button';
      b.append(picture(c.images[0], '', 'ph-mini'));
      const body = el('span', 'body');
      body.append(el('span', 'name', c.name));
      const price = el('span', 'price', money(c.price, spec.currency));
      if (c.regularPrice !== null) price.append(' ', el('s', 'was', money(c.regularPrice, spec.currency)));
      body.append(price);
      const rating = el('span', 'rating');
      rating.append(stars(c.reviews.average, true));
      rating.append(el('span', 'n', c.reviews.average !== null ? `${c.reviews.average.toFixed(1)} (${c.reviews.count.toLocaleString()})` : 'no reviews'));
      body.append(rating, verdictPill(c.reviews, true));
      b.append(body);
      const diff = c.price - p.price;
      const cmp = el('span', `diff ${diff < 0 ? 'less' : diff > 0 ? 'more' : 'same'}`,
        Math.abs(diff) < 0.005 ? 'same price' : `${money(Math.abs(diff), spec.currency)} ${diff < 0 ? 'less' : 'more'}`);
      b.append(cmp);
      b.setAttribute('aria-label', `View ${c.name}, ${money(c.price, spec.currency)}`);
      b.addEventListener('click', () => select(c.sku, spec.options.some((o) => o.sku === p.sku) ? p.sku : state.from));
      strip.append(b);
    }
    section.append(strip);
    return section;
  }

  /** The panel for the product `state.selected`; `select(sku, from)` shows another. */
  function detail(spec, state, select) {
    const p = spec.bySku.get(state.selected);
    const panel = el('section', 'cedar-products-detail');
    panel.setAttribute('role', 'tabpanel');
    panel.setAttribute('aria-label', p.name);
    const isOption = spec.options.some((o) => o.sku === p.sku);
    if (!isOption && state.from && spec.bySku.has(state.from)) {
      const back = el('button', 'cedar-products-back');
      back.type = 'button';
      back.innerHTML = ICONS.back;
      back.append(`Back to ${spec.bySku.get(state.from).name}`);
      back.addEventListener('click', () => select(state.from, null));
      panel.append(back);
    }
    const grid = el('div', 'grid');
    grid.append(gallery(p));

    const info = el('div', 'cedar-products-info');
    const badges = spec.badges.get(p.sku) || [];
    if (badges.length || p.category) {
      const row = el('div', 'badges');
      for (const label of badges) row.append(el('span', 'badge', label));
      if (p.category) row.append(el('span', 'cat', p.category));
      info.append(row);
    }
    const name = el('div', 'name', p.name);
    name.setAttribute('role', 'heading');
    name.setAttribute('aria-level', '3');
    info.append(name);
    const sub = [p.brand, p.model ? `Model ${p.model}` : '', p.color].filter(Boolean).join(' · ');
    if (sub) info.append(el('div', 'sub', sub));
    info.append(priceLine(p, spec.currency, true));
    info.append(availabilityChips(p));
    if (p.url) {
      const link = el('a', 'cedar-products-link');
      link.href = p.url;
      link.target = '_blank';
      link.rel = 'noopener noreferrer';
      link.referrerPolicy = 'no-referrer';
      link.append(`View at ${spec.store}`);
      link.innerHTML += ICONS.out;
      info.append(link);
    }
    info.append(reviewsBlock(p));
    if (p.description) info.append(el('p', 'cedar-products-desc', p.description));
    if (p.features.length) {
      const f = el('section', 'cedar-products-features');
      f.append(el('div', 'cedar-products-label', 'Key features'));
      const list = el('ul');
      for (const feature of p.features) list.append(el('li', '', feature));
      f.append(list);
      info.append(f);
    }
    if (p.specs.length) {
      const s = el('section', 'cedar-products-specs');
      s.append(el('div', 'cedar-products-label', 'Specs'));
      const dl = el('dl');
      for (const [k, v] of p.specs) dl.append(el('dt', '', k), el('dd', '', v));
      s.append(dl);
      info.append(s);
    }
    grid.append(info);
    panel.append(grid);
    const strip = comparableStrip(p, spec, state, select);
    if (strip) panel.append(strip);
    return panel;
  }

  // -- the card --------------------------------------------------------------------------------

  /** The card for `spec`. Its tiles appear one after another the first time it is on the page. */
  function element(spec) {
    const card = el('div', 'cedar-products');
    card.setAttribute('role', 'group');
    card.setAttribute('aria-label', `Products: ${spec.query}`);

    const head = el('div', 'cedar-products-head');
    const icon = el('span', 'cedar-products-icon');
    icon.innerHTML = ICONS.bag;
    const titles = el('div');
    titles.append(el('div', 'cedar-products-title', spec.query));
    const parts = [spec.store, `${spec.options.length} of ${spec.total.toLocaleString()} result${spec.total === 1 ? '' : 's'}`];
    if (spec.sort) parts.push(spec.sort);
    titles.append(el('div', 'cedar-products-sub', parts.join(' · ')));
    head.append(icon, titles);
    card.append(head);

    const state = { selected: spec.options[0].sku, from: null };
    const tiles = el('div', 'cedar-products-options');
    tiles.setAttribute('role', 'tablist');
    tiles.setAttribute('aria-label', 'Options');
    const tileEls = spec.options.map((p, i) => tile(p, spec, i));
    tiles.append(...tileEls);
    card.append(tiles);

    let panel = detail(spec, state, select);
    card.append(panel);

    function select(sku, from) {
      if (!spec.bySku.has(sku)) return;
      state.selected = sku;
      state.from = from;
      tileEls.forEach((t) => t.setAttribute('aria-selected', String(t.dataset.sku === sku)));
      const next = detail(spec, state, select);
      panel.replaceWith(next);
      panel = next;
    }
    tileEls.forEach((t, i) => {
      t.addEventListener('click', () => select(t.dataset.sku, null));
      t.addEventListener('keydown', (e) => {
        if (e.key !== 'ArrowRight' && e.key !== 'ArrowLeft') return;
        e.preventDefault();
        const j = (i + (e.key === 'ArrowRight' ? 1 : -1) + tileEls.length) % tileEls.length;
        tileEls[j].focus();
        select(tileEls[j].dataset.sku, null);
      });
    });
    tileEls.forEach((t) => t.setAttribute('aria-selected', String(t.dataset.sku === state.selected)));

    const foot = el('div', 'cedar-products-foot');
    if (spec.disclaimer) foot.append(el('span', '', spec.disclaimer));
    if (!Number.isNaN(spec.asOf)) {
      foot.append(el('span', 'when', `As of ${new Date(spec.asOf).toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' })}`));
    }
    card.append(foot);

    // The reveal runs once, when the card is first visible, so a card scrolled to later still plays
    // it, and one moved around the page doesn't play it again.
    card.classList.add('reveal');
    const seen = new IntersectionObserver((entries) => {
      if (!entries.some((e) => e.isIntersecting)) return;
      seen.disconnect();
      card.classList.add('play');
      setTimeout(() => card.classList.remove('reveal', 'play'), spec.options.length * TILE_STAGGER_MS + 800);
    });
    seen.observe(card);
    return card;
  }

  return { parse, element };
})();

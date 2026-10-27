'use strict';

// Product display for cedar's `search_products` tool, shared by every UI. The tool's result is a
// JSON object, {"query", "needs"?, "store", "currency", "total", "considered"?: {"pages",
// "products"}, "as_of", "method"?: "model" | "ratings", "summary"?, "note"?, "options": [{"sku",
// "name", "brand"?, "model"?, "price"?, "currency"?, "price_checked"?, "regular_price"?,
// "savings_pct"?, "url"?, "store"?, "images": [url], "description"?, "features", "specs",
// "category"?, "availability"?: {"online"?, "in_store"?, "free_shipping"?}, "reviews": {"average",
// "count", "adjusted", "verdict", "confidence", "summary"}, "badges"?: [text], "why"?, "pros"?,
// "cons"?, "best_for"?, "sources"?: [n], "comparable": [sku]}], "others": [...], "picks":
// {"top_pick"?, "best_value"?, "best_rated"?, ...}, "sources"?: [{"n", "title", "url", "host"}],
// "disclaimer"}, and this turns it into a card:
//
//   - the model's recommendation in a sentence or three, when it weighed the options
//   - a tile per option (its photo, its badges, price and savings, stars and the number of
//     reviews, the verdict), which picks the product shown below
//   - the selected product: a photo gallery, the price (marked when it comes from review text,
//     not a store page), what is known of its stock, a link to the store, why it fits and who it
//     suits, its pros and cons, the reading of its reviews (stars, the adjusted rating on a bar,
//     the verdict and how much to trust it), the description, key features, specs, and the
//     sources behind it
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
  const VERDICTS = {
    excellent: 'product.verdict.excellent', good: 'product.verdict.good', mixed: 'product.verdict.mixed',
    poor: 'product.verdict.poor', few: 'product.verdict.few', none: 'product.verdict.none',
  };
  const CONFIDENCE = { high: 'product.confidence.high', medium: 'product.confidence.medium', low: 'product.confidence.low' };
  const PICKS = [
    ['top_pick', 'product.pick.topPick'], ['best_value', 'product.pick.bestValue'],
    ['best_rated', 'product.pick.bestRated'], ['most_popular', 'product.pick.mostPopular'],
  ];
  const MAX_SOURCES = 40;
  const SORTS = {
    best_selling: 'product.sort.bestSelling', rating: 'product.sort.rating',
    price_low: 'product.sort.priceLow', price_high: 'product.sort.priceHigh',
  };
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
    const rawPrice = num(p.price);
    const price = rawPrice !== null && rawPrice >= 0 ? rawPrice : null;
    if (!sku || !name) return null;
    const images = (Array.isArray(p.images) ? p.images : [p.image]).map(https).filter(Boolean).slice(0, MAX_IMAGES);
    const regular = num(p.regular_price);
    const a = p.availability && typeof p.availability === 'object' ? p.availability : {};
    const known = (v) => (typeof v === 'boolean' ? v : null);
    const list = (v, max, n) => (Array.isArray(v) ? v : []).map((x) => str(x, max)).filter(Boolean).slice(0, n);
    return {
      sku,
      name,
      brand: str(p.brand, 80),
      model: str(p.model, 80),
      color: str(p.color, 60),
      condition: str(p.condition, 40),
      price,
      currency: /^[A-Za-z]{3}$/.test(str(p.currency, 3)) ? str(p.currency, 3).toUpperCase() : '',
      priceChecked: p.price_checked !== false,
      regularPrice: price !== null && regular !== null && regular > price ? regular : null,
      savingsPct: num(p.savings_pct),
      url: https(p.url),
      store: str(p.store, 80),
      images,
      description: str(p.description, MAX_LONG),
      features: (Array.isArray(p.features) ? p.features : []).map((f) => str(f)).filter(Boolean).slice(0, MAX_FEATURES),
      specs: (Array.isArray(p.specs) ? p.specs : []).map((s) => [str(s?.name, 80), str(s?.value)]).filter(([n, v]) => n && v).slice(0, MAX_SPECS),
      category: str(p.category, 80),
      availability: { online: known(a.online), inStore: known(a.in_store), freeShipping: known(a.free_shipping) },
      badges: Array.isArray(p.badges) ? list(p.badges, 40, 4) : null,
      why: str(p.why, MAX_LONG),
      pros: list(p.pros, 120, 4),
      cons: list(p.cons, 120, 4),
      bestFor: str(p.best_for, 160),
      sources: (Array.isArray(p.sources) ? p.sources : []).map(num).filter((n) => n !== null).slice(0, 6),
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
      if (known.has(sku)) badges.set(sku, [...(badges.get(sku) || []), tr(label)]);
    }
    // An option that names its own badges (the tool's web search does) keeps exactly those.
    for (const p of options) if (p.badges) badges.set(p.sku, p.badges);
    const sources = new Map();
    for (const s of (Array.isArray(data.sources) ? data.sources : []).slice(0, MAX_SOURCES)) {
      const n = num(s?.n);
      const title = str(s?.title) || str(s?.host, 80);
      if (n !== null && title) sources.set(n, { title, url: https(s.url), host: str(s.host, 80) });
    }
    const considered = data.considered && typeof data.considered === 'object'
      ? { pages: Math.max(0, Math.round(num(data.considered.pages) ?? 0)), products: Math.max(0, Math.round(num(data.considered.products) ?? 0)) }
      : null;
    return {
      query: str(data.query, 160) || tr('product.defaultQuery'),
      store: str(data.store, 60) || tr('product.theStore'),
      storeNamed: Boolean(str(data.store, 60)),
      currency: str(data.currency, 8) || 'USD',
      total: Math.max(options.length + others.length, Math.round(num(data.total) ?? 0)),
      asOf: Date.parse(str(data.as_of, 40)),
      sort: SORTS[data.sort] ? tr(SORTS[data.sort]) : '',
      considered,
      summary: str(data.summary, 800),
      note: str(data.note, 600),
      sources,
      options,
      others,
      bySku,
      badges,
      disclaimer: str(data.disclaimer, MAX_LONG),
    };
  }

  // -- formatting ----------------------------------------------------------------------------

  function money(x, currency) {
    if (x === null) return '–';
    const opts = { minimumFractionDigits: 2, maximumFractionDigits: 2 };
    if (/^[A-Z]{3}$/.test(currency)) {
      try {
        return x.toLocaleString(cedarI18n.locale, { ...opts, style: 'currency', currency });
      } catch { /* not a currency code Intl knows: the number and the code */ }
    }
    return `${x.toLocaleString(cedarI18n.locale, opts)} ${currency}`;
  }

  const count = (n) => tr('product.reviews', { count: n });
  /** A rating such as 4.5, with the locale's decimal sign. */
  const rating1 = (v) => cedarI18n.number(v, { minimumFractionDigits: 1, maximumFractionDigits: 1 });
  /** The currency a product's price is in: its own, else the card's. */
  const cur = (p, spec) => p.currency || spec.currency;

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
    wrap.setAttribute('aria-label', value === null ? tr('product.notRated') : tr('product.starsAria', { rating: rating1(value) }));
    const base = el('span', 'base');
    base.innerHTML = ICONS.star.repeat(5);
    const fill = el('span', 'fill');
    fill.innerHTML = ICONS.star.repeat(5);
    fill.style.width = `${((value ?? 0) / 5) * 100}%`;
    wrap.append(base, fill);
    return wrap;
  }

  function verdictPill(reviews, small) {
    const pill = el('span', `cedar-products-verdict ${reviews.verdict}${small ? ' small' : ''}`, tr(VERDICTS[reviews.verdict]));
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
    if (p.price === null) {
      row.append(el('span', 'none', tr('product.priceAtStore')));
      return row;
    }
    const now = el('span', `now${p.priceChecked ? '' : ' unchecked'}`, `${p.priceChecked ? '' : '≈ '}${money(p.price, currency)}`);
    if (!p.priceChecked) now.title = tr('product.unchecked');
    row.append(now);
    if (p.regularPrice !== null) {
      const was = el('s', 'was', money(p.regularPrice, currency));
      was.setAttribute('aria-label', tr('product.was', { price: money(p.regularPrice, currency) }));
      row.append(was);
      const pct = p.savingsPct ?? Math.round((1 - p.price / p.regularPrice) * 100);
      if (pct >= 1) row.append(el('span', 'save', tr('product.save', { pct: (Math.round(pct) / 100).toLocaleString(cedarI18n.locale, { style: 'percent' }) })));
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
    b.append(priceLine(p, cur(p, spec), false));
    const r = p.reviews;
    const rating = el('span', 'rating');
    rating.append(stars(r.average, true));
    if (r.average !== null) rating.append(el('span', 'avg', rating1(r.average)));
    rating.append(el('span', 'n', r.count ? `(${cedarI18n.number(r.count)})` : tr('product.noReviews')));
    b.append(rating, verdictPill(r, true));
    b.setAttribute('aria-label', `${p.name}, ${p.price === null ? tr('product.priceAtStoreLower') : money(p.price, cur(p, spec))}, ${r.average !== null ? tr('product.starsFrom', { rating: rating1(r.average), reviews: count(r.count) }) : tr('product.noReviews')}${badges.length ? `, ${badges.join(', ')}` : ''}`);
    return b;
  }

  // -- the detail panel ------------------------------------------------------------------------

  function gallery(p) {
    const g = el('div', 'cedar-products-gallery');
    const main = el('div', 'main');
    const thumbs = el('div', 'thumbs');
    thumbs.setAttribute('role', 'group');
    thumbs.setAttribute('aria-label', tr('product.photosOf', { name: p.name }));
    const buttons = [];
    let current = 0;
    function show(i) {
      current = (i + p.images.length) % p.images.length;
      main.replaceChildren(picture(p.images[current], tr('product.photoOf', { name: p.name, n: current + 1, total: p.images.length }), 'ph-main'));
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
      b.setAttribute('aria-label', tr('product.photoN', { n: i + 1 }));
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
    box.append(el('div', 'cedar-products-label', tr('product.reviewsLabel')));
    const head = el('div', 'head');
    head.append(stars(r.average, false));
    if (r.average !== null) {
      head.append(el('span', 'avg', rating1(r.average)));
      head.append(el('span', 'n', tr('product.fromReviews', { reviews: count(r.count) })));
    } else {
      head.append(el('span', 'n', tr('product.noCustomerRating')));
    }
    box.append(head);
    if (r.adjusted !== null && r.count > 0) {
      const bar = el('div', 'bar');
      bar.setAttribute('role', 'img');
      bar.setAttribute('aria-label', tr('product.adjustedAria', { rating: rating1(r.adjusted) }));
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
      legend.append(el('span', 'k', tr('product.adjustedNote')), el('strong', 'v', tr('product.outOf5', { rating: rating1(r.adjusted) })));
      bar.append(legend, track);
      box.append(bar);
    }
    const line = el('div', 'verdict-line');
    line.append(verdictPill(r, false));
    if (r.count > 0) line.append(el('span', 'conf', tr(CONFIDENCE[r.confidence])));
    box.append(line);
    if (r.summary && r.average !== null) box.append(el('p', 'summary', r.summary));
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
    // Only what the store said: a page that doesn't mention stock gets no chip for it.
    if (p.availability.online !== null) chip(tr(p.availability.online ? 'product.inStock' : 'product.outOfStock'), p.availability.online);
    if (p.availability.inStore) chip(tr('product.inStore'), true);
    if (p.availability.freeShipping) chip(tr('product.freeShipping'), true);
    if (p.condition && !/^new$/i.test(p.condition)) chip(p.condition, false);
    return row.childElementCount ? row : null;
  }

  function comparableStrip(p, spec, state, select) {
    const items = p.comparable.map((s) => spec.bySku.get(s)).filter(Boolean);
    if (!items.length) return null;
    const section = el('section', 'cedar-products-comparable');
    section.append(el('div', 'cedar-products-label', tr('product.comparable')));
    const strip = el('div', 'strip');
    for (const c of items) {
      const b = el('button', 'mini');
      b.type = 'button';
      b.append(picture(c.images[0], '', 'ph-mini'));
      const body = el('span', 'body');
      body.append(el('span', 'name', c.name));
      const price = el('span', 'price', c.price === null ? tr('product.priceAtStore') : `${c.priceChecked ? '' : '≈ '}${money(c.price, cur(c, spec))}`);
      if (c.regularPrice !== null) price.append(' ', el('s', 'was', money(c.regularPrice, cur(c, spec))));
      body.append(price);
      const rating = el('span', 'rating');
      rating.append(stars(c.reviews.average, true));
      rating.append(el('span', 'n', c.reviews.average !== null ? `${rating1(c.reviews.average)} (${cedarI18n.number(c.reviews.count)})` : tr('product.noReviews')));
      body.append(rating, verdictPill(c.reviews, true));
      b.append(body);
      // A difference only means something between two prices in the same currency.
      if (c.price !== null && p.price !== null && cur(c, spec) === cur(p, spec)) {
        const diff = c.price - p.price;
        b.append(el('span', `diff ${diff < 0 ? 'less' : diff > 0 ? 'more' : 'same'}`,
          Math.abs(diff) < 0.005 ? tr('product.samePrice') : tr(diff < 0 ? 'product.less' : 'product.more', { price: money(Math.abs(diff), cur(c, spec)) })));
      }
      b.setAttribute('aria-label', tr('product.viewAria', { name: c.name, price: c.price === null ? tr('product.priceAtStoreLower') : money(c.price, cur(c, spec)) }));
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
      back.append(tr('product.backTo', { name: spec.bySku.get(state.from).name }));
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
    const sub = [p.brand, p.model ? tr('product.model', { model: p.model }) : '', p.color].filter(Boolean).join(' · ');
    if (sub) info.append(el('div', 'sub', sub));
    info.append(priceLine(p, cur(p, spec), true));
    const chips = availabilityChips(p);
    if (chips) info.append(chips);
    if (p.url) {
      const link = el('a', 'cedar-products-link');
      link.href = p.url;
      link.target = '_blank';
      link.rel = 'noopener noreferrer';
      link.referrerPolicy = 'no-referrer';
      link.append(p.store || spec.storeNamed ? tr('product.viewAt', { store: p.store || spec.store }) : tr('product.viewAtStore'));
      link.innerHTML += ICONS.out;
      info.append(link);
    }
    if (p.why || p.bestFor) {
      const why = el('section', 'cedar-products-why');
      why.append(el('div', 'cedar-products-label', tr('product.whyFits')));
      if (p.why) why.append(el('p', '', p.why));
      if (p.bestFor) {
        const who = el('p', 'for');
        who.append(el('strong', '', tr('product.bestFor')), p.bestFor);
        why.append(who);
      }
      info.append(why);
    }
    if (p.pros.length || p.cons.length) {
      const pc = el('div', 'cedar-products-proscons');
      for (const [label, items, cls] of [[tr('product.pros'), p.pros, 'pros'], [tr('product.cons'), p.cons, 'cons']]) {
        if (!items.length) continue;
        const col = el('section', cls);
        col.append(el('div', 'cedar-products-label', label));
        const ul = el('ul');
        for (const item of items) ul.append(el('li', '', item));
        col.append(ul);
        pc.append(col);
      }
      info.append(pc);
    }
    info.append(reviewsBlock(p));
    if (p.description) info.append(el('p', 'cedar-products-desc', p.description));
    if (p.features.length) {
      const f = el('section', 'cedar-products-features');
      f.append(el('div', 'cedar-products-label', tr('product.features')));
      const list = el('ul');
      for (const feature of p.features) list.append(el('li', '', feature));
      f.append(list);
      info.append(f);
    }
    if (p.specs.length) {
      const s = el('section', 'cedar-products-specs');
      s.append(el('div', 'cedar-products-label', tr('product.specs')));
      const dl = el('dl');
      for (const [k, v] of p.specs) dl.append(el('dt', '', k), el('dd', '', v));
      s.append(dl);
      info.append(s);
    }
    const cited = p.sources.map((n) => [n, spec.sources.get(n)]).filter(([, s]) => s);
    if (cited.length) {
      const src = el('section', 'cedar-products-sources');
      src.append(el('div', 'cedar-products-label', tr('product.sources')));
      const ol = el('ol');
      for (const [n, s] of cited) {
        const li = el('li');
        li.append(el('span', 'n', `${n}`));
        if (s.url) {
          const a = el('a', '', s.title);
          a.href = s.url;
          a.target = '_blank';
          a.rel = 'noopener noreferrer';
          a.referrerPolicy = 'no-referrer';
          li.append(a);
        } else {
          li.append(el('span', '', s.title));
        }
        if (s.host) li.append(el('span', 'host', s.host));
        ol.append(li);
      }
      src.append(ol);
      info.append(src);
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
    card.setAttribute('aria-label', tr('product.aria', { query: spec.query }));

    const head = el('div', 'cedar-products-head');
    const icon = el('span', 'cedar-products-icon');
    icon.innerHTML = ICONS.bag;
    const titles = el('div');
    titles.append(el('div', 'cedar-products-title', spec.query));
    const parts = spec.considered
      ? [tr('product.pickedFrom', {
        n: spec.options.length,
        products: tr('product.products', { count: spec.considered.products }),
        pages: tr('product.pages', { count: spec.considered.pages }),
      })]
      : [spec.store, tr('product.nOfTotal', { n: spec.options.length, count: spec.total })];
    if (spec.sort) parts.push(spec.sort);
    titles.append(el('div', 'cedar-products-sub', parts.join(' · ')));
    head.append(icon, titles);
    card.append(head);
    if (spec.summary) {
      const verdict = el('div', 'cedar-products-summary');
      verdict.append(el('div', 'cedar-products-label', tr('product.recommendation')), el('p', '', spec.summary));
      card.append(verdict);
    }
    if (spec.note) card.append(el('div', 'cedar-products-note', spec.note));

    const state = { selected: spec.options[0].sku, from: null };
    const tiles = el('div', 'cedar-products-options');
    tiles.setAttribute('role', 'tablist');
    tiles.setAttribute('aria-label', tr('product.options'));
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
      foot.append(el('span', 'when', tr('product.asOf', { date: new Date(spec.asOf).toLocaleString(cedarI18n.locale, { dateStyle: 'medium', timeStyle: 'short' }) })));
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

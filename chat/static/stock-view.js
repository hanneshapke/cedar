'use strict';

// Stock display for cedar's `compare_stocks` tool, shared by both UIs (/v1 and /v2). The tool's
// result is a JSON object, {"range", "benchmark"?: {"symbol", "name", "in_symbols"},
// "benchmark_series"?: {"symbol", "name", "dates", "close"}, "as_of", "symbols": [{"symbol", "name",
// "type", "currency", "price", "change", "change_pct", "returns", "indicators", "rating": {"call",
// "lean", "score", "max_score", "confidence", "summary", "factors", "watch"}, "chart": {"dates",
// "close"}, ...}], "news": [{"title", "publisher", "url", "published", "thumbnail"?, "symbols"}],
// "news_note"?, "unavailable"?, "disclaimer"}, and this turns it into a card:
//
//   - a quote tile per symbol (price, day's move, rating), which also picks the symbol shown below
//   - a line chart with a crosshair: % change against the benchmark, or one symbol's price
//   - the selected symbol's rating: the call on a sell–hold–buy gauge, the summary, every factor
//     with its score and reasoning, what would change the view, and the key numbers
//   - the returns of every symbol side by side, and the news, filterable by symbol
//
//   cedarStocks.parse(output) -> spec | null   the validated result, or null if it isn't one
//   cedarStocks.element(spec) -> HTMLElement   the card
//
// It needs nothing from the app. Everything in the result came from the network or the model, so
// text is only ever set as text, and links and pictures are only used if they are https URLs.

const cedarStocks = (() => {
  const MAX_SYMBOLS = 6;
  const MAX_POINTS = 2000;
  const MAX_NEWS = 24;
  const MAX_TEXT = 300;
  const MAX_LONG = 1200;
  const SVG_NS = 'http://www.w3.org/2000/svg';
  const CHART_HEIGHT = 240;
  const PAD = { top: 12, right: 60, bottom: 26, left: 52 };
  const RANGES = [['1M', 30], ['3M', 91], ['6M', 182], ['1Y', 365], ['2Y', 730], ['5Y', 1826]];
  const RANGE_KEYS = { '1mo': '1M', '3mo': '3M', '6mo': '6M', '1y': '1Y', '2y': '2Y', '5y': '5Y' };
  const DAY_MS = 86400000;
  const RETURN_COLS = [['1w', '1W'], ['1m', '1M'], ['3m', '3M'], ['ytd', 'YTD'], ['6m', '6M'], ['1y', '1Y']];
  const CALLS = { buy: 'stock.call.buy', hold: 'stock.call.hold', sell: 'stock.call.sell' };
  /** The periods' names as shown (the RANGES and RETURN_COLS labels above are their keys). */
  const PERIOD = {
    '1W': 'stock.period.1W', '1M': 'stock.period.1M', '3M': 'stock.period.3M', '6M': 'stock.period.6M',
    '1Y': 'stock.period.1Y', '2Y': 'stock.period.2Y', '5Y': 'stock.period.5Y', YTD: 'stock.period.YTD',
  };
  const period = (k) => tr(PERIOD[k]);
  const CONFIDENCE = { low: 'stock.confidence.low', medium: 'stock.confidence.medium', high: 'stock.confidence.high' };

  function el(tag, className, text) {
    const e = document.createElement(tag);
    if (className) e.className = className;
    if (text !== undefined) e.textContent = text;
    return e;
  }

  function svg(tag, attrs = {}) {
    const e = document.createElementNS(SVG_NS, tag);
    for (const [k, v] of Object.entries(attrs)) e.setAttribute(k, String(v));
    return e;
  }

  const str = (v, max = MAX_TEXT) => (typeof v === 'string' && v.trim() ? v.trim().slice(0, max) : '');
  const num = (v) => (typeof v === 'number' && Number.isFinite(v) ? v : null);
  const https = (v) => (typeof v === 'string' && /^https:\/\/[^\s"'<>]+$/.test(v) ? v : '');
  const day = (s) => (typeof s === 'string' && /^\d{4}-\d{2}-\d{2}$/.test(s) ? Date.parse(`${s}T00:00:00Z`) : NaN);

  /** `{name, t: [ms], v: [close]}` from a `chart` object, or null when it has under two points. */
  function series(chart, fallbackName) {
    if (!chart || !Array.isArray(chart.dates) || !Array.isArray(chart.close)) return null;
    const t = [];
    const v = [];
    const n = Math.min(chart.dates.length, chart.close.length, MAX_POINTS);
    for (let i = 0; i < n; i++) {
      const ms = day(chart.dates[i]);
      const c = num(chart.close[i]);
      if (Number.isNaN(ms) || c === null || c <= 0) continue;
      if (t.length && ms <= t[t.length - 1]) continue;
      t.push(ms);
      v.push(c);
    }
    return t.length >= 2 ? { t, v, name: str(chart.name) || fallbackName } : null;
  }

  function parseRating(r) {
    if (!r || !CALLS[r.call]) return null;
    const factors = (Array.isArray(r.factors) ? r.factors : []).slice(0, 12)
      .map((f) => ({
        name: str(f?.name, 80), headline: str(f?.headline, 80), detail: str(f?.detail, MAX_LONG),
        score: Math.max(-2, Math.min(2, Math.round(num(f?.score) ?? 0))),
      }))
      .filter((f) => f.name && f.detail);
    const max = Math.max(1, Math.round(num(r.max_score) ?? 9));
    return {
      call: r.call,
      lean: str(r.lean, 40),
      score: Math.max(-max, Math.min(max, Math.round(num(r.score) ?? 0))),
      max,
      confidence: ['low', 'medium', 'high'].includes(r.confidence) ? r.confidence : 'low',
      summary: str(r.summary, MAX_LONG),
      factors,
      watch: (Array.isArray(r.watch) ? r.watch : []).map((w) => str(w, MAX_LONG)).filter(Boolean).slice(0, 6),
    };
  }

  /** The stocks in a `compare_stocks` result, or null if `output` isn't one with a priced symbol. */
  function parse(output) {
    let data;
    try { data = JSON.parse(output); } catch { return null; }
    if (!data || typeof data !== 'object' || !Array.isArray(data.symbols)) return null;
    const stocks = data.symbols.slice(0, MAX_SYMBOLS).map((s) => {
      const symbol = str(s?.symbol, 20);
      const price = num(s?.price);
      const line = series(s?.chart, symbol);
      if (!symbol || price === null || !line) return null;
      const ind = s.indicators && typeof s.indicators === 'object' ? s.indicators : {};
      const returns = {};
      for (const [k] of RETURN_COLS) returns[k] = num(s.returns?.[k]);
      return {
        symbol,
        name: str(s.name, 120) || symbol,
        kind: str(s.type, 30),
        currency: str(s.currency, 8),
        exchange: str(s.exchange, 60),
        price,
        change: num(s.change),
        changePct: num(s.change_pct),
        dayHigh: num(s.day_high),
        dayLow: num(s.day_low),
        high52: num(s.fifty_two_week_high),
        low52: num(s.fifty_two_week_low),
        returns,
        ind: {
          sma50: num(ind.sma50), sma200: num(ind.sma200), rsi: num(ind.rsi14),
          vol: num(ind.volatility_pct), drawdown: num(ind.max_drawdown_pct), beta: num(ind.beta),
          relative: num(ind.vs_benchmark_6m_pts),
        },
        rating: parseRating(s.rating),
        line,
      };
    }).filter(Boolean);
    if (!stocks.length) return null;

    let benchmark = null;
    const b = data.benchmark;
    if (b && str(b.symbol, 20)) {
      const line = b.in_symbols ? null : series(data.benchmark_series, str(b.symbol, 20));
      benchmark = { symbol: str(b.symbol, 20), name: str(b.name, 120) || str(b.symbol, 20), line };
    }
    const symbols = new Set(stocks.map((s) => s.symbol));
    const news = (Array.isArray(data.news) ? data.news : []).slice(0, MAX_NEWS).map((n) => {
      const url = https(n?.url);
      const title = str(n?.title);
      if (!url || !title) return null;
      return {
        title, url,
        publisher: str(n.publisher, 80),
        published: Date.parse(str(n.published, 40)),
        thumbnail: https(n.thumbnail),
        symbols: (Array.isArray(n.symbols) ? n.symbols : []).filter((s) => symbols.has(s)),
      };
    }).filter(Boolean);
    const unavailable = (Array.isArray(data.unavailable) ? data.unavailable : [])
      .map((u) => [str(u?.symbol, 20), str(u?.reason, 200)])
      .filter(([s]) => s);
    return {
      range: RANGE_KEYS[data.range] || '1Y',
      asOf: Date.parse(str(data.as_of, 40)),
      stocks,
      benchmark,
      news,
      newsNote: str(data.news_note, 400),
      unavailable,
      disclaimer: str(data.disclaimer, 600),
    };
  }

  // -- formatting ----------------------------------------------------------------------------

  /** `x` in `currency`, with as many decimals as a price the size of `scale` (default `x`) needs. */
  function money(x, currency, scale = x) {
    if (x === null) return '–';
    const digits = Math.abs(scale) >= 1000 ? 0 : Math.abs(scale) >= 1 ? 2 : 4;
    const opts = { minimumFractionDigits: digits, maximumFractionDigits: digits };
    // Only an ISO code: Intl would read "GBp" (pence) as pounds.
    if (/^[A-Z]{3}$/.test(currency)) {
      try {
        return x.toLocaleString(cedarI18n.locale, { ...opts, style: 'currency', currency });
      } catch { /* not a currency code Intl knows: the number and the code */ }
    }
    const n = x.toLocaleString(cedarI18n.locale, opts);
    return currency ? `${n} ${currency}` : n;
  }

  /** `x` percent (not a fraction) written the locale's way, with `min` to `max` decimals. */
  const percent = (x, min, max = min) => (x / 100).toLocaleString(cedarI18n.locale, {
    style: 'percent', minimumFractionDigits: min, maximumFractionDigits: max,
  });
  const signed = (x, digits = 1) => (x === null ? '–' : `${x > 0 ? '+' : x < 0 ? '−' : ''}${percent(Math.abs(x), digits)}`);
  const direction = (x) => (x === null || Math.abs(x) < 0.005 ? 'flat' : x > 0 ? 'up' : 'down');

  function shortDate(ms, span) {
    const d = new Date(ms);
    const opts = span > 400 * DAY_MS
      ? { month: 'short', year: '2-digit', timeZone: 'UTC' }
      : { month: 'short', day: 'numeric', timeZone: 'UTC' };
    return d.toLocaleDateString(cedarI18n.locale, opts);
  }

  function ago(ms) {
    if (Number.isNaN(ms)) return '';
    const mins = Math.round((Date.now() - ms) / 60000);
    if (mins < 1) return tr('stock.justNow');
    if (mins < 60) return tr('stock.minAgo', { count: mins });
    const hours = Math.round(mins / 60);
    if (hours < 24) return tr('stock.hoursAgo', { count: hours });
    const days = Math.round(hours / 24);
    if (days < 14) return tr('stock.daysAgo', { count: days });
    return new Date(ms).toLocaleDateString(cedarI18n.locale, { month: 'short', day: 'numeric' });
  }

  const ICONS = {
    buy: '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M6 15l6-6 6 6"/></svg>',
    sell: '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M6 9l6 6 6-6"/></svg>',
    hold: '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M6 12h12"/></svg>',
    chart: '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M4 19h16M5 15l4-5 4 3 6-7"/></svg>',
  };

  function verdictPill(rating, small) {
    const pill = el('span', `cedar-stocks-call ${rating.call}${small ? ' small' : ''}`);
    pill.innerHTML = ICONS[rating.call];
    pill.append(tr(CALLS[rating.call]));
    return pill;
  }

  // -- the chart -----------------------------------------------------------------------------

  /** Round tick values between lo and hi, about `count` of them. */
  function ticks(lo, hi, count) {
    const span = hi - lo || Math.abs(hi) || 1;
    const raw = span / count;
    const mag = 10 ** Math.floor(Math.log10(raw));
    const step = [1, 2, 2.5, 5, 10].map((m) => m * mag).find((s) => s >= raw) || raw;
    const out = [];
    for (let v = Math.ceil(lo / step) * step; v <= hi + step * 1e-9; v += step) out.push(+v.toFixed(10));
    return out;
  }

  /** The index of the last point at or before `t`, or -1. */
  function at(line, t) {
    let lo = 0;
    let hi = line.t.length - 1;
    if (t < line.t[0]) return -1;
    while (lo < hi) {
      const mid = (lo + hi + 1) >> 1;
      if (line.t[mid] <= t) lo = mid; else hi = mid - 1;
    }
    return lo;
  }

  /**
   * The chart: a toolbar (range, and % change or price), the plot, and its legend. `state.selected`
   * is the symbol shown in price mode; `onChange` re-renders the plot.
   */
  function chartSection(spec, state) {
    const section = el('section', 'cedar-stocks-chart');
    const bar = el('div', 'cedar-stocks-toolbar');
    const ranges = el('div', 'cedar-stocks-seg');
    ranges.setAttribute('role', 'group');
    ranges.setAttribute('aria-label', tr('stock.chartPeriod'));
    const modes = el('div', 'cedar-stocks-seg');
    modes.setAttribute('role', 'group');
    modes.setAttribute('aria-label', tr('stock.chartValues'));
    bar.append(ranges, modes);

    const lines = spec.stocks.map((s, i) => ({ key: s.symbol, label: s.symbol, line: s.line, slot: i + 1 }));
    if (spec.benchmark?.line) {
      lines.push({ key: spec.benchmark.symbol, label: spec.benchmark.name, line: spec.benchmark.line, slot: 0 });
    }
    const first = Math.min(...lines.map((l) => l.line.t[0]));
    const last = Math.max(...lines.map((l) => l.line.t[l.line.t.length - 1]));
    // Offer the periods the data covers (with some slack for weekends and holidays).
    const offered = RANGES.filter(([k, days]) => k === spec.range || last - days * DAY_MS >= first - 7 * DAY_MS);
    state.range = offered.some(([k]) => k === spec.range) ? spec.range : offered[offered.length - 1][0];
    state.mode = lines.length > 1 ? 'pct' : 'price';

    const wrap = el('div', 'cedar-stocks-plot');
    const tip = el('div', 'cedar-stocks-tip');
    tip.hidden = true;
    const legend = el('div', 'cedar-stocks-legend');
    section.append(bar, wrap, legend);

    const button = (group, label, active, onClick) => {
      const b = el('button', '', label);
      b.type = 'button';
      b.setAttribute('aria-pressed', String(active));
      b.addEventListener('click', () => {
        for (const o of group.children) o.setAttribute('aria-pressed', String(o === b));
        onClick();
      });
      group.append(b);
      return b;
    };
    for (const [k] of offered) button(ranges, period(k), k === state.range, () => { state.range = k; draw(); });
    button(modes, tr('stock.pctChange'), state.mode === 'pct', () => { state.mode = 'pct'; draw(); });
    button(modes, tr('stock.price'), state.mode === 'price', () => { state.mode = 'price'; draw(); });

    let width = 0;
    let animate = true;

    function draw() {
      width = Math.max(260, Math.round(wrap.clientWidth || 600));
      const days = RANGES.find(([k]) => k === state.range)[1];
      const from = last - days * DAY_MS;
      const shown = state.mode === 'price' ? lines.filter((l) => l.key === state.selected) : lines;
      // Each series from the first point inside the period; % change counts from that point.
      const data = shown.map((l) => {
        let start = at(l.line, from);
        if (start < 0 || l.line.t[start] < from) start += 1;
        start = Math.max(0, Math.min(start, l.line.t.length - 2));
        const base = l.line.v[start];
        const pts = [];
        for (let i = start; i < l.line.t.length; i++) {
          pts.push([l.line.t[i], state.mode === 'pct' ? (l.line.v[i] / base - 1) * 100 : l.line.v[i]]);
        }
        return { ...l, pts, base };
      });
      const t0 = Math.min(...data.map((d) => d.pts[0][0]));
      const t1 = last;
      let lo = Math.min(...data.flatMap((d) => d.pts.map((p) => p[1])));
      let hi = Math.max(...data.flatMap((d) => d.pts.map((p) => p[1])));
      if (state.mode === 'pct') { lo = Math.min(lo, 0); hi = Math.max(hi, 0); }
      const padY = (hi - lo || Math.abs(hi) || 1) * 0.08;
      lo -= padY; hi += padY;
      const yt = ticks(lo, hi, 4);
      lo = Math.min(lo, yt[0]); hi = Math.max(hi, yt[yt.length - 1]);
      const plotW = width - PAD.left - PAD.right;
      const plotH = CHART_HEIGHT - PAD.top - PAD.bottom;
      const x = (t) => PAD.left + ((t - t0) / (t1 - t0 || 1)) * plotW;
      const y = (v) => PAD.top + (1 - (v - lo) / (hi - lo || 1)) * plotH;
      const stock = spec.stocks.find((s) => s.symbol === state.selected);
      const fmt = state.mode === 'pct' ? (v) => signed(v) : (v) => money(v, stock?.currency || '');

      const root = svg('svg', { viewBox: `0 0 ${width} ${CHART_HEIGHT}`, width, height: CHART_HEIGHT, class: 'cedar-stocks-svg' });
      root.setAttribute('role', 'img');
      root.setAttribute('aria-label', state.mode === 'pct'
        ? tr('stock.pctAria', { range: period(state.range), values: data.map((d) => `${d.label} ${signed(d.pts[d.pts.length - 1][1])}`).join(', ') })
        : tr('stock.priceAria', { symbol: state.selected, range: period(state.range) }));
      const grid = svg('g', { class: 'grid' });
      for (const v of yt) {
        grid.append(svg('line', { x1: PAD.left, x2: width - PAD.right, y1: y(v), y2: y(v), class: v === 0 && state.mode === 'pct' ? 'zero' : '' }));
        const label = svg('text', { x: PAD.left - 8, y: y(v) + 4, class: 'axis', 'text-anchor': 'end' });
        label.textContent = state.mode === 'pct' ? `${v > 0 ? '+' : v < 0 ? '-' : ''}${percent(Math.abs(v), 0, 2)}` : money(v, stock?.currency || '');
        grid.append(label);
      }
      const xt = width < 480 ? 3 : 5;
      for (let i = 0; i < xt; i++) {
        const t = t0 + ((t1 - t0) * (i + 0.5)) / xt;
        const label = svg('text', { x: x(t), y: CHART_HEIGHT - 6, class: 'axis', 'text-anchor': 'middle' });
        label.textContent = shortDate(t, t1 - t0);
        grid.append(label);
      }
      root.append(grid);

      if (state.mode === 'price' && data[0]) {
        const d = data[0];
        const path = d.pts.map(([t, v], i) => `${i ? 'L' : 'M'}${x(t).toFixed(1)},${y(v).toFixed(1)}`).join('');
        const fill = svg('path', { d: `${path}L${x(d.pts[d.pts.length - 1][0]).toFixed(1)},${PAD.top + plotH}L${x(d.pts[0][0]).toFixed(1)},${PAD.top + plotH}Z`, class: `area s${d.slot}` });
        root.append(fill);
      }
      // The benchmark first, so the symbols draw over it.
      for (const d of [...data].sort((a, b) => (a.slot === 0 ? -1 : b.slot === 0 ? 1 : 0))) {
        const path = svg('path', {
          d: d.pts.map(([t, v], i) => `${i ? 'L' : 'M'}${x(t).toFixed(1)},${y(v).toFixed(1)}`).join(''),
          class: `line s${d.slot}${animate ? ' draw' : ''}`,
        });
        // A path length of 1 lets the draw-in animate any line; the dashed benchmark keeps its own.
        if (d.slot) path.setAttribute('pathLength', '1');
        root.append(path);
        const [lt, lv] = d.pts[d.pts.length - 1];
        root.append(svg('circle', { cx: x(lt), cy: y(lv), r: 3.5, class: `end s${d.slot}` }));
      }
      // End labels for up to four lines, nudged apart so they don't overlap.
      if (data.length <= 4) {
        const labels = data.map((d) => ({ d, y: y(d.pts[d.pts.length - 1][1]) })).sort((a, b) => a.y - b.y);
        for (let i = 1; i < labels.length; i++) labels[i].y = Math.max(labels[i].y, labels[i - 1].y + 13);
        const over = labels.length ? labels[labels.length - 1].y - (PAD.top + plotH) : 0;
        if (over > 0) labels.forEach((l) => { l.y -= over; });
        for (const l of labels) {
          const label = svg('text', { x: width - PAD.right + 8, y: l.y + 4, class: 'end-label' });
          label.textContent = l.d.slot === 0 && l.d.label.length > 10 ? tr('stock.indexShort') : l.d.label;
          root.append(label);
        }
      }
      const cross = svg('line', { y1: PAD.top, y2: PAD.top + plotH, class: 'cross' });
      cross.style.display = 'none';
      const dots = data.map((d) => {
        const c = svg('circle', { r: 4, class: `dot s${d.slot}` });
        c.style.display = 'none';
        return c;
      });
      root.append(cross, ...dots);
      const hit = svg('rect', { x: PAD.left, y: PAD.top, width: plotW, height: plotH, class: 'hit' });
      root.append(hit);
      root.setAttribute('tabindex', '0');

      // The crosshair snaps to the nearest day of the first line and lists every line there.
      const ref = data[0].pts;
      let cursor = -1;
      function show(i) {
        cursor = Math.max(0, Math.min(ref.length - 1, i));
        const t = ref[cursor][0];
        cross.setAttribute('x1', x(t));
        cross.setAttribute('x2', x(t));
        cross.style.display = '';
        tip.replaceChildren(el('div', 'when', new Date(t).toLocaleDateString(cedarI18n.locale, { year: 'numeric', month: 'short', day: 'numeric', timeZone: 'UTC' })));
        data.forEach((d, k) => {
          const j = at({ t: d.pts.map((p) => p[0]) }, t);
          if (j < 0) { dots[k].style.display = 'none'; return; }
          const [pt, pv] = d.pts[j];
          dots[k].setAttribute('cx', x(pt));
          dots[k].setAttribute('cy', y(pv));
          dots[k].style.display = '';
          const row = el('div', 'row');
          row.append(el('span', `key s${d.slot}`), el('strong', '', fmt(pv)), el('span', 'who', d.label));
          tip.append(row);
        });
        tip.hidden = false;
        const left = x(t);
        tip.style.left = `${Math.min(Math.max(left + 12, 0), width - tip.offsetWidth - 4)}px`;
        if (left + 12 + tip.offsetWidth > width - 4) tip.style.left = `${Math.max(0, left - tip.offsetWidth - 12)}px`;
      }
      function hide() {
        cross.style.display = 'none';
        dots.forEach((d) => { d.style.display = 'none'; });
        tip.hidden = true;
        cursor = -1;
      }
      function nearest(clientX) {
        const box = root.getBoundingClientRect();
        const px = ((clientX - box.left) / box.width) * width;
        const t = t0 + ((px - PAD.left) / plotW) * (t1 - t0);
        let best = 0;
        for (let i = 1; i < ref.length; i++) if (Math.abs(ref[i][0] - t) < Math.abs(ref[best][0] - t)) best = i;
        return best;
      }
      hit.addEventListener('pointermove', (e) => show(nearest(e.clientX)));
      hit.addEventListener('pointerdown', (e) => show(nearest(e.clientX)));
      hit.addEventListener('pointerleave', hide);
      root.addEventListener('blur', hide);
      root.addEventListener('keydown', (e) => {
        const step = { ArrowLeft: -1, ArrowRight: 1, Home: -1e9, End: 1e9 }[e.key];
        if (step === undefined) { if (e.key === 'Escape') hide(); return; }
        e.preventDefault();
        show(cursor < 0 ? ref.length - 1 : cursor + step);
      });

      wrap.replaceChildren(root, tip);
      animate = false;

      // The legend: a line key, the name, and the move over the period.
      legend.replaceChildren(...data.map((d) => {
        const item = el('span', 'item');
        const change = state.mode === 'pct' ? d.pts[d.pts.length - 1][1] : (d.pts[d.pts.length - 1][1] / d.base - 1) * 100;
        const val = el('span', `chg ${direction(change)}`, signed(change));
        item.append(el('span', `key s${d.slot}`), el('span', 'name', d.slot === 0 ? tr('stock.benchmarkLabel', { name: d.label }) : d.label), val);
        return item;
      }));
    }
    state.redrawChart = draw;
    // Drawn once the card is on the page and has a width, and again when that width changes.
    let seenWidth = 0;
    new ResizeObserver(() => {
      const w = Math.round(wrap.clientWidth);
      if (w && w !== seenWidth) { seenWidth = w; draw(); }
    }).observe(wrap);
    return section;
  }

  // -- the rating ----------------------------------------------------------------------------

  /** The score on a sell–hold–buy track from -max to +max. */
  function gauge(rating) {
    const g = el('div', 'cedar-stocks-gauge');
    const track = el('div', 'track');
    const span = rating.max * 2;
    const pos = (v) => `${((v + rating.max) / span) * 100}%`;
    // A score of 3 or more is a buy and -3 or less a sell, so the zones meet halfway between.
    const edge = Math.min(2.5, rating.max);
    for (const [cls, a, b] of [['sell', -rating.max, -edge], ['hold', -edge, edge], ['buy', edge, rating.max]]) {
      const z = el('span', `zone ${cls}`);
      z.style.left = pos(a);
      z.style.width = `${((b - a) / span) * 100}%`;
      track.append(z);
    }
    const marker = el('span', `marker ${rating.call}`);
    marker.style.left = pos(rating.score);
    marker.title = tr('stock.scoreOf', { score: `${rating.score > 0 ? '+' : ''}${rating.score}`, max: rating.max });
    track.append(marker);
    const labels = el('div', 'labels');
    labels.append(el('span', '', tr(CALLS.sell)), el('span', '', tr(CALLS.hold)), el('span', '', tr(CALLS.buy)));
    g.append(track, labels);
    g.setAttribute('role', 'img');
    g.setAttribute('aria-label', tr('stock.gaugeAria', { score: rating.score, max: rating.max }));
    return g;
  }

  /** A factor's score as a bar growing left (against) or right (for) from the middle. */
  function scoreBar(score) {
    const bar = el('span', `cedar-stocks-score ${score > 0 ? 'for' : score < 0 ? 'against' : 'neutral'}`);
    const fill = el('span', 'fill');
    fill.style.width = `${(Math.abs(score) / 2) * 50}%`;
    bar.append(fill);
    const label = el('span', 'n', score > 0 ? `+${score}` : score < 0 ? `−${-score}` : '0');
    const wrap = el('span', 'cedar-stocks-scorewrap');
    wrap.append(bar, label);
    const word = tr(score >= 2 ? 'stock.strongFor' : score === 1 ? 'stock.for' : score === 0 ? 'stock.neutral'
      : score === -1 ? 'stock.against' : 'stock.strongAgainst');
    wrap.setAttribute('aria-label', tr('stock.scoreAria', { score, word }));
    wrap.title = word;
    return wrap;
  }

  function stat(label, value, extra) {
    const s = el('div', 'cedar-stocks-stat');
    s.append(el('span', 'k', label), el('span', 'v', value));
    if (extra) s.append(extra);
    return s;
  }

  /** A low–high range with a marker where the price is. */
  function rangeBar(low, high, price) {
    const bar = el('span', 'cedar-stocks-range');
    const p = Math.max(0, Math.min(1, (price - low) / (high - low || 1)));
    const mark = el('span', 'mark');
    mark.style.left = `${p * 100}%`;
    bar.append(mark);
    return bar;
  }

  function ratingPanel(stock, spec) {
    const panel = el('div', 'cedar-stocks-rating');
    const r = stock.rating;
    const top = el('div', 'cedar-stocks-verdict');
    const head = el('div', 'cedar-stocks-verdict-head');
    if (r) {
      head.append(verdictPill(r));
      const meta = el('div', 'cedar-stocks-verdict-meta');
      const title = el('div', 'who');
      title.append(el('strong', '', stock.symbol), ` ${stock.name}`);
      const conf = el('div', 'conf');
      const meter = el('span', `meter ${r.confidence}`);
      meter.append(el('i'), el('i'), el('i'));
      conf.append(meter, tr(CONFIDENCE[r.confidence]));
      if (r.lean) conf.append(el('span', 'lean', r.lean));
      meta.append(title, conf);
      head.append(meta);
      top.append(head, gauge(r));
      if (r.summary) top.append(el('p', 'summary', r.summary));
    } else {
      head.append(el('strong', '', stock.symbol), ` ${tr('stock.noRating', { name: stock.name })}`);
      top.append(head);
    }
    panel.append(top);

    if (r?.factors.length) {
      const sec = el('section', 'cedar-stocks-why');
      sec.append(el('div', 'cedar-stocks-label', tr('stock.why')));
      const list = el('ol', 'cedar-stocks-factors');
      for (const f of r.factors) {
        const li = el('li', f.score > 0 ? 'for' : f.score < 0 ? 'against' : 'neutral');
        const line = el('div', 'line');
        const name = el('span', 'name', f.name);
        line.append(name);
        if (f.headline) line.append(el('span', 'headline', f.headline));
        line.append(scoreBar(f.score));
        li.append(line, el('p', 'detail', f.detail));
        list.append(li);
      }
      sec.append(list);
      panel.append(sec);
    }
    if (r?.watch.length) {
      const sec = el('section', 'cedar-stocks-watch');
      sec.append(el('div', 'cedar-stocks-label', tr('stock.watch')));
      const list = el('ul');
      for (const w of r.watch) list.append(el('li', '', w));
      sec.append(list);
      panel.append(sec);
    }

    const stats = el('div', 'cedar-stocks-stats');
    const m = (v) => money(v, stock.currency);
    if (stock.low52 !== null && stock.high52 !== null) {
      stats.append(stat(tr('stock.range52'), `${m(stock.low52)} – ${m(stock.high52)}`, rangeBar(stock.low52, stock.high52, stock.price)));
    }
    if (stock.dayLow !== null && stock.dayHigh !== null) {
      stats.append(stat(tr('stock.dayRange'), `${m(stock.dayLow)} – ${m(stock.dayHigh)}`, rangeBar(stock.dayLow, stock.dayHigh, stock.price)));
    }
    const i = stock.ind;
    if (i.sma50 !== null) stats.append(stat(tr('stock.avg50'), m(i.sma50)));
    if (i.sma200 !== null) stats.append(stat(tr('stock.avg200'), m(i.sma200)));
    if (i.rsi !== null) stats.append(stat(tr('stock.rsi'), cedarI18n.number(Math.round(i.rsi))));
    if (i.vol !== null) stats.append(stat(tr('stock.volatility'), tr('stock.perYear', { value: percent(i.vol, 0) })));
    if (i.drawdown !== null) stats.append(stat(tr('stock.drawdown'), signed(i.drawdown)));
    if (i.beta !== null) stats.append(stat(tr('stock.beta', { name: spec.benchmark?.name || tr('stock.index') }), cedarI18n.number(i.beta, { minimumFractionDigits: 2, maximumFractionDigits: 2 })));
    if (stats.children.length) panel.append(stats);
    return panel;
  }

  // -- returns and news ------------------------------------------------------------------------

  function returnsTable(spec) {
    const sec = el('section', 'cedar-stocks-returns');
    sec.append(el('div', 'cedar-stocks-label', tr('stock.returns')));
    const scroll = el('div', 'scroll');
    const table = el('table');
    const head = el('tr');
    head.append(el('th', '', ''));
    for (const [, label] of RETURN_COLS) head.append(el('th', '', period(label)));
    const thead = el('thead');
    thead.append(head);
    const body = el('tbody');
    spec.stocks.forEach((s, idx) => {
      const row = el('tr');
      const th = el('th');
      th.scope = 'row';
      th.append(el('span', `key s${idx + 1}`), s.symbol);
      row.append(th);
      for (const [k] of RETURN_COLS) {
        const v = s.returns[k];
        const td = el('td', direction(v), signed(v));
        // A tint scaled to the size of the move, capped at ±40%.
        if (v !== null) td.style.setProperty('--mag', String(Math.min(1, Math.abs(v) / 40)));
        row.append(td);
      }
      body.append(row);
    });
    table.append(thead, body);
    scroll.append(table);
    sec.append(scroll);
    return sec;
  }

  function newsSection(spec) {
    const sec = el('section', 'cedar-stocks-news');
    const top = el('div', 'cedar-stocks-label');
    top.append(el('span', '', tr('stock.news')));
    sec.append(top);
    if (!spec.news.length) {
      sec.append(el('p', 'cedar-stocks-note', spec.newsNote || tr('stock.noNews')));
      return sec;
    }
    const withSymbols = spec.stocks.filter((s) => spec.news.some((n) => n.symbols.includes(s.symbol)));
    const list = el('ul', 'cedar-stocks-articles');
    const items = spec.news.map((n) => {
      const li = el('li');
      const a = el('a');
      a.href = n.url;
      a.target = '_blank';
      a.rel = 'noopener noreferrer';
      a.referrerPolicy = 'no-referrer';
      const body = el('div', 'body');
      body.append(el('span', 'title', n.title));
      const meta = el('span', 'meta');
      if (n.publisher) meta.append(el('span', 'pub', n.publisher));
      const when = ago(n.published);
      if (when) {
        const time = el('time', '', when);
        time.dateTime = new Date(n.published).toISOString();
        meta.append(time);
      }
      for (const s of n.symbols) meta.append(el('span', 'tag', s));
      body.append(meta);
      a.append(body);
      if (n.thumbnail) {
        const img = el('img', 'thumb');
        img.alt = '';
        img.loading = 'lazy';
        img.decoding = 'async';
        img.referrerPolicy = 'no-referrer';
        img.addEventListener('error', () => img.remove());
        img.src = n.thumbnail;
        a.append(img);
      }
      li.append(a);
      list.append(li);
      return { li, symbols: n.symbols };
    });
    // Filter chips when the headlines cover more than one symbol.
    if (withSymbols.length > 1) {
      const chips = el('div', 'cedar-stocks-chips');
      chips.setAttribute('role', 'group');
      chips.setAttribute('aria-label', tr('stock.newsFor'));
      const chip = (label, sym) => {
        const b = el('button', '', label);
        b.type = 'button';
        b.setAttribute('aria-pressed', String(!sym));
        b.addEventListener('click', () => {
          for (const c of chips.children) c.setAttribute('aria-pressed', String(c === b));
          for (const it of items) it.li.hidden = sym ? !it.symbols.includes(sym) : false;
        });
        chips.append(b);
      };
      chip(tr('stock.all'));
      for (const s of withSymbols) chip(s.symbol, s.symbol);
      top.append(chips);
    }
    sec.append(list);
    if (spec.newsNote) sec.append(el('p', 'cedar-stocks-note', spec.newsNote));
    return sec;
  }

  // -- the card --------------------------------------------------------------------------------

  function element(spec) {
    const card = el('div', 'cedar-stocks');
    card.setAttribute('role', 'group');
    const names = spec.stocks.map((s) => s.symbol);
    card.setAttribute('aria-label', tr('stock.aria', { names: names.join(', ') }));
    const state = { selected: spec.stocks[0].symbol };

    const head = el('div', 'cedar-stocks-head');
    const icon = el('span', 'cedar-stocks-icon');
    icon.innerHTML = ICONS.chart;
    const titles = el('div');
    const title = el('div', 'cedar-stocks-title', names.length === 1 ? `${names[0]} · ${spec.stocks[0].name}` : names.join(' vs '));
    title.setAttribute('role', 'heading');
    title.setAttribute('aria-level', '3');
    const sub = [];
    if (spec.benchmark) sub.push(tr('stock.benchmark', { name: spec.benchmark.name }));
    if (!Number.isNaN(spec.asOf)) {
      sub.push(tr('stock.asOf', { date: new Date(spec.asOf).toLocaleString(cedarI18n.locale, { month: 'short', day: 'numeric', hour: 'numeric', minute: '2-digit' }) }));
    }
    titles.append(title, el('div', 'cedar-stocks-sub', sub.join(' · ')));
    head.append(icon, titles);
    card.append(head);

    // Quote tiles: one per symbol; picking one shows its rating (and its price in price mode).
    const tiles = el('div', 'cedar-stocks-tiles');
    tiles.setAttribute('role', 'tablist');
    tiles.setAttribute('aria-label', tr('stock.symbol'));
    const ratingHolder = el('div', 'cedar-stocks-rating-holder');
    ratingHolder.setAttribute('role', 'tabpanel');
    const panels = new Map();
    const tileEls = spec.stocks.map((s, idx) => {
      const t = el('button', 'cedar-stocks-tile');
      t.type = 'button';
      t.setAttribute('role', 'tab');
      const top = el('span', 'top');
      top.append(el('span', `key s${idx + 1}`), el('span', 'sym', s.symbol));
      if (s.rating) top.append(verdictPill(s.rating, true));
      const price = el('span', 'price', money(s.price, s.currency));
      const dir = direction(s.changePct);
      const chg = el('span', `chg ${dir}`);
      chg.textContent = `${dir === 'up' ? '▲' : dir === 'down' ? '▼' : '■'} ${s.change !== null ? money(Math.abs(s.change), s.currency, Math.min(s.price, 999)) : ''} (${signed(s.changePct, 2)})`;
      t.append(top, el('span', 'name', s.name), price, chg);
      t.addEventListener('click', () => select(s.symbol));
      t.addEventListener('keydown', (e) => {
        const step = { ArrowRight: 1, ArrowDown: 1, ArrowLeft: -1, ArrowUp: -1 }[e.key];
        if (!step) return;
        e.preventDefault();
        const next = spec.stocks[(idx + step + spec.stocks.length) % spec.stocks.length].symbol;
        select(next);
        tileEls[spec.stocks.findIndex((x) => x.symbol === next)].focus();
      });
      tiles.append(t);
      return t;
    });
    function select(symbol) {
      state.selected = symbol;
      spec.stocks.forEach((s, i) => {
        tileEls[i].setAttribute('aria-selected', String(s.symbol === symbol));
        tileEls[i].tabIndex = s.symbol === symbol ? 0 : -1;
      });
      if (!panels.has(symbol)) panels.set(symbol, ratingPanel(spec.stocks.find((s) => s.symbol === symbol), spec));
      ratingHolder.replaceChildren(panels.get(symbol));
      if (state.mode === 'price') state.redrawChart?.();
    }
    card.append(tiles, chartSection(spec, state), ratingHolder);
    select(state.selected);

    if (spec.stocks.length > 1 || spec.stocks[0].returns['1y'] !== null) card.append(returnsTable(spec));
    card.append(newsSection(spec));

    const foot = el('div', 'cedar-stocks-foot');
    for (const [s, reason] of spec.unavailable) foot.append(el('p', 'cedar-stocks-note', tr('stock.unavailable', { symbol: s, reason })));
    foot.append(el('p', 'disclaimer', `${spec.disclaimer || tr('stock.notAdvice')} ${tr('stock.data', { source: 'Yahoo Finance' })}`));
    card.append(foot);
    return card;
  }

  return { parse, element };
})();

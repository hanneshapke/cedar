'use strict';

// Chart display for cedar's `generate_chart` tool, shared by every UI. The tool saves a picture
// (drawn with matplotlib in the sandbox, for files and PDFs); in the chat the same data is drawn
// here instead, from the call's input, {"path", "kind", "labels"?, "series": [{"name"?, "values"}],
// "title"?, "x_label"?, "y_label"?, "stacked"?, "bins"?, "log_scale"?}, as a card:
//
//   - a heading (the title, the kind, and the file it was saved as)
//   - a legend whose entries hide and show their series
//   - the plot, drawn as SVG to the card's width, with a tooltip: a crosshair over lines and
//     areas, a highlighted category over bars, the nearest point of a scatter, a slice of a pie
//     (arrow keys move it too)
//   - the data as a table, folded under the plot
//
//   cedarChart.parse(output, input) -> spec | null   the validated input, or null if it isn't one
//   cedarChart.element(spec) -> HTMLElement          the card
//
// It needs nothing from the app. The input came from the model, so text is only ever set as text.

const cedarChart = (() => {
  const KINDS = ['line', 'bar', 'barh', 'area', 'scatter', 'pie', 'histogram'];
  const KIND_NAMES = {
    line: 'Line chart', bar: 'Bar chart', barh: 'Bar chart', area: 'Area chart',
    scatter: 'Scatter plot', pie: 'Pie chart', histogram: 'Histogram',
  };
  const MAX_SERIES = 20;
  const MAX_VALUES = 100000;
  const SLOTS = 8; // the categorical palette; series past it are drawn in the neutral "other" grey
  const MAX_SLICES = 8; // a pie folds the smallest slices past this into "Other"
  const MAX_TABLE_ROWS = 200;
  const SVG_NS = 'http://www.w3.org/2000/svg';
  const ICON = '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M4 4v16h16"/><path d="M8 16v-4M12 16V8M16 16v-6"/></svg>';
  const PIE_ICON = '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M12 3a9 9 0 1 0 9 9h-9z"/><path d="M15 3.5A9 9 0 0 1 20.5 9H15z"/></svg>';

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

  const str = (v, max = 200) => (typeof v === 'string' && v.trim() ? v.trim().slice(0, max) : '');
  const num = (v) => (typeof v === 'number' && Number.isFinite(v) ? v : null);
  const slotClass = (slot) => (slot < SLOTS ? `s${slot + 1}` : 'so');

  /** Every `step`-th index of `n`, always keeping the last, so long series stay drawable. */
  function thin(n, max) {
    if (n <= max) return null;
    const step = n / max;
    const keep = [];
    for (let i = 0; i < max; i++) keep.push(Math.floor(i * step));
    if (keep[keep.length - 1] !== n - 1) keep.push(n - 1);
    return keep;
  }

  /** The chart in a `generate_chart` call's input, or null if it can't be drawn. */
  function parse(_output, input) {
    if (!input || typeof input !== 'object') return null;
    const kind = KINDS.includes(input.kind) ? input.kind : null;
    if (!kind || !Array.isArray(input.series) || !input.series.length) return null;
    const series = [];
    for (const [i, s] of input.series.slice(0, MAX_SERIES).entries()) {
      if (!s || !Array.isArray(s.values) || !s.values.length) return null;
      if (s.values.some((v) => v !== null && num(v) === null)) return null;
      series.push({ name: str(s.name, 80) || `Series ${i + 1}`, named: Boolean(str(s.name)), values: s.values.slice(0, MAX_VALUES).map(num), slot: i });
    }
    let labels = [];
    if (kind !== 'histogram') {
      if (!Array.isArray(input.labels) || !input.labels.length) return null;
      labels = input.labels.slice(0, MAX_VALUES).map((l) => (typeof l === 'number' && Number.isFinite(l) ? l : typeof l === 'string' ? l.slice(0, 120) : null));
      if (labels.some((l) => l === null)) return null;
      if (series.some((s) => s.values.length !== labels.length)) return null;
      if (kind === 'scatter' && !labels.every((l) => typeof l === 'number')) return null;
    }
    if (kind === 'pie') {
      if (series.length !== 1 || series[0].values.some((v) => v === null || v < 0)) return null;
      if (!series[0].values.some((v) => v > 0)) return null;
    }

    // Long series are thinned to what a card can show; the saved file has every point.
    let thinned = false;
    if (kind !== 'histogram' && kind !== 'pie') {
      const keep = thin(labels.length, { bar: 300, barh: 120, scatter: 3000 }[kind] || 1500);
      if (keep) {
        thinned = true;
        labels = keep.map((i) => labels[i]);
        for (const s of series) s.values = keep.map((i) => s.values[i]);
      }
    }
    const bins = num(input.bins);
    // A shaded range on the value axis, such as a target range.
    let band = null;
    if (input.band && typeof input.band === 'object' && kind !== 'pie') {
      const from = num(input.band.from);
      const to = num(input.band.to);
      if (from !== null && to !== null && from !== to) {
        band = { from: Math.min(from, to), to: Math.max(from, to), label: str(input.band.label, 40) };
      }
    }
    return {
      kind,
      path: str(input.path, 300),
      title: str(input.title),
      xLabel: str(input.x_label, 120),
      yLabel: str(input.y_label, 120),
      labels,
      series,
      stacked: input.stacked === true && ['bar', 'barh', 'area', 'histogram'].includes(kind),
      bins: bins && bins >= 1 ? Math.min(500, Math.round(bins)) : null,
      log: input.log_scale === true && kind !== 'pie' && kind !== 'histogram',
      band,
      thinned,
    };
  }

  // -- numbers ---------------------------------------------------------------------------------

  const trimZeros = (s) => (s.includes('.') ? s.replace(/\.?0+$/, '') : s);

  /** A short number for an axis: 1.2K, 3.4M, 0.25. */
  function compact(v, step = Math.abs(v)) {
    const a = Math.abs(v);
    for (const [div, suffix] of [[1e12, 'T'], [1e9, 'B'], [1e6, 'M'], [1e3, 'K']]) {
      if (a >= div && (a >= 1e4 || suffix !== 'K')) {
        const digits = Math.max(0, Math.min(2, -Math.floor(Math.log10((step || a) / div))));
        return `${v < 0 ? '−' : ''}${trimZeros((a / div).toFixed(digits))}${suffix}`;
      }
    }
    const digits = step > 0 ? Math.max(0, Math.min(6, -Math.floor(Math.log10(step)))) : 2;
    const s = trimZeros(a.toFixed(digits));
    return `${v < 0 && s !== '0' ? '−' : ''}${s}`;
  }

  /** A number as it reads in a tooltip or table. */
  function full(v) {
    if (v === null || v === undefined) return '–';
    const a = Math.abs(v);
    const digits = a >= 1000 ? 0 : a >= 100 ? 1 : a >= 1 ? 2 : a === 0 ? 0 : 4;
    return v.toLocaleString(undefined, { maximumFractionDigits: digits }).replace(/^-/, '−');
  }

  const labelText = (l) => (typeof l === 'number' ? full(l) : l);

  /** Round tick values between lo and hi, about `count` of them. */
  function ticks(lo, hi, count) {
    const span = hi - lo || Math.abs(hi) || 1;
    const raw = span / count;
    const mag = 10 ** Math.floor(Math.log10(raw));
    const step = [1, 2, 2.5, 5, 10].map((m) => m * mag).find((s) => s >= raw) || raw;
    const out = [];
    for (let v = Math.ceil(lo / step - 1e-9) * step; v <= hi + step * 1e-9; v += step) out.push(+v.toFixed(10));
    return { values: out, step };
  }

  /** A value axis: the domain rounded out to ticks, and a scale onto [r0, r1]. */
  function valueAxis(lo, hi, r0, r1, log, count, zero) {
    if (log) {
      const l0 = Math.floor(Math.log10(lo));
      let l1 = Math.ceil(Math.log10(hi));
      if (l1 === l0) l1 += 1;
      const values = [];
      const every = Math.max(1, Math.ceil((l1 - l0) / 6));
      for (let p = l0; p <= l1; p += every) values.push(10 ** p);
      const scale = (v) => r0 + ((Math.log10(Math.max(v, 10 ** l0)) - l0) / (l1 - l0)) * (r1 - r0);
      return { scale, ticks: values, step: 0, base: 10 ** l0, lo: 10 ** l0, hi: 10 ** l1 };
    }
    if (zero) { lo = Math.min(lo, 0); hi = Math.max(hi, 0); }
    if (lo === hi) { lo -= Math.abs(lo) * 0.1 || 1; hi += Math.abs(hi) * 0.1 || 1; }
    // Breathing room past the data, but a zero baseline stays where it is.
    const pad = (hi - lo) * 0.06;
    if (!zero || lo !== 0) lo -= pad;
    if (!zero || hi !== 0) hi += pad;
    const t = ticks(lo, hi, count);
    lo = Math.min(lo, t.values[0]);
    hi = Math.max(hi, t.values[t.values.length - 1]);
    const scale = (v) => r0 + ((v - lo) / (hi - lo || 1)) * (r1 - r0);
    return { scale, ticks: t.values, step: t.step, base: Math.max(lo, Math.min(0, hi)), lo, hi };
  }

  /** Histogram bins over every series' samples: `{edges, counts: [per series]}`. */
  function histogram(spec) {
    const samples = spec.series.map((s) => s.values.filter((v) => v !== null));
    const all = samples.flat();
    if (!all.length) return null;
    let lo = Math.min(...all);
    let hi = Math.max(...all);
    if (lo === hi) { lo -= 0.5; hi += 0.5; }
    const n = spec.bins || Math.max(5, Math.min(40, Math.ceil(Math.log2(all.length) + 1)));
    const width = (hi - lo) / n;
    const edges = Array.from({ length: n + 1 }, (_, i) => lo + i * width);
    const counts = samples.map((vals) => {
      const c = new Array(n).fill(0);
      for (const v of vals) c[Math.min(n - 1, Math.floor((v - lo) / width))] += 1;
      return c;
    });
    return { edges, counts };
  }

  // -- shapes ----------------------------------------------------------------------------------

  /** A bar from (x, y) of size w × h with the end away from the baseline rounded. */
  function barPath(x, y, w, h, end) {
    const r = Math.max(0, Math.min(4, w / 2, h / 2));
    if (r < 0.5) return `M${x},${y}h${w}v${h}h${-w}Z`;
    switch (end) {
      case 'top': return `M${x},${y + h}V${y + r}Q${x},${y} ${x + r},${y}H${x + w - r}Q${x + w},${y} ${x + w},${y + r}V${y + h}Z`;
      case 'bottom': return `M${x},${y}V${y + h - r}Q${x},${y + h} ${x + r},${y + h}H${x + w - r}Q${x + w},${y + h} ${x + w},${y + h - r}V${y}Z`;
      case 'right': return `M${x},${y}H${x + w - r}Q${x + w},${y} ${x + w},${y + r}V${y + h - r}Q${x + w},${y + h} ${x + w - r},${y + h}H${x}Z`;
      case 'left': return `M${x + w},${y}H${x + r}Q${x},${y} ${x},${y + r}V${y + h - r}Q${x},${y + h} ${x + r},${y + h}H${x + w}Z`;
      default: return `M${x},${y}h${w}v${h}h${-w}Z`;
    }
  }

  /** An SVG path through points, broken at gaps (null). */
  function linePath(points) {
    let d = '';
    let pen = false;
    for (const p of points) {
      if (!p) { pen = false; continue; }
      d += `${pen ? 'L' : 'M'}${p[0].toFixed(1)},${p[1].toFixed(1)}`;
      pen = true;
    }
    return d;
  }

  const textWidth = (s, size = 11.5) => String(s).length * size * 0.58;
  const clip = (s, px, size = 11.5) => {
    const max = Math.max(1, Math.floor(px / (size * 0.58)));
    return s.length > max ? `${s.slice(0, Math.max(1, max - 1))}…` : s;
  };

  // -- the cartesian plot ----------------------------------------------------------------------

  /**
   * Draws a line, area, bar, barh, scatter, or histogram plot at `width` into `wrap`, with `tip`
   * as its tooltip. `hidden` holds the slots the legend turned off.
   */
  function drawPlot(spec, wrap, tip, width, hidden, animate) {
    const { kind } = spec;
    const horizontal = kind === 'barh';
    const visible = spec.series.filter((s) => !hidden.has(s.slot));
    const hist = kind === 'histogram' ? histogram({ ...spec, series: visible }) : null;
    // Categories (bars, histogram bins, category lines) or a numeric x (scatter, numeric lines).
    const numericX = kind === 'scatter' || ((kind === 'line' || kind === 'area') && spec.labels.every((l) => typeof l === 'number'));
    const cats = hist ? hist.counts[0].map((_, i) => `${full(hist.edges[i])} – ${full(hist.edges[i + 1])}`) : spec.labels;
    const data = hist ? visible.map((s, k) => ({ ...s, values: hist.counts[k] })) : visible;
    const n = cats.length;
    const stacked = spec.stacked && data.length > 1;
    const barLike = kind === 'bar' || kind === 'barh' || kind === 'histogram';

    // The values the value axis has to hold.
    let lo = Infinity;
    let hi = -Infinity;
    if (stacked) {
      for (let i = 0; i < n; i++) {
        let pos = 0;
        let neg = 0;
        for (const s of data) { const v = s.values[i] ?? 0; if (v >= 0) pos += v; else neg += v; }
        hi = Math.max(hi, pos); lo = Math.min(lo, neg);
      }
    } else {
      for (const s of data) for (const v of s.values) if (v !== null && (!spec.log || v > 0)) { lo = Math.min(lo, v); hi = Math.max(hi, v); }
    }
    if (!Number.isFinite(lo)) { lo = spec.log ? 1 : 0; hi = spec.log ? 10 : 1; }
    const target = spec.band && !(spec.log && spec.band.from <= 0) ? spec.band : null;
    if (target) { lo = Math.min(lo, target.from); hi = Math.max(hi, target.to); }

    const narrow = width < 440;
    const height = horizontal ? Math.max(180, Math.min(900, n * (data.length > 1 && !stacked ? 14 * data.length + 10 : 30) + 50)) : narrow ? 240 : 290;
    const valueTickCount = horizontal ? (narrow ? 4 : 6) : 5;
    // The value axis is built twice: once to measure its labels, then to fit the room left.
    const probe = valueAxis(lo, hi, 0, 1, spec.log, valueTickCount, barLike || kind === 'area');
    const valueLabel = (v) => compact(v, probe.step || Math.abs(v));
    const catLabelWidth = horizontal ? Math.min(narrow ? 96 : 160, Math.max(...cats.map((c) => textWidth(labelText(c)))) + 4) : 0;
    const endLabels = (kind === 'line' || kind === 'area') && data.length > 1 && data.length <= 4 && !narrow;
    const pad = {
      top: 14,
      right: endLabels ? Math.min(110, Math.max(...data.map((s) => textWidth(s.name))) + 16) : horizontal ? 18 : 14,
      bottom: 26 + (spec.xLabel ? 18 : 0),
      left: (horizontal ? catLabelWidth + 10 : Math.max(...probe.ticks.map((v) => textWidth(valueLabel(v)))) + 12) + (spec.yLabel ? 20 : 0),
    };
    const plotW = Math.max(40, width - pad.left - pad.right);
    const plotH = height - pad.top - pad.bottom;
    const axis = horizontal
      ? valueAxis(lo, hi, pad.left, pad.left + plotW, spec.log, valueTickCount, true)
      : valueAxis(lo, hi, pad.top + plotH, pad.top, spec.log, valueTickCount, barLike || kind === 'area');
    const v = axis.scale;

    // Where each category or x value sits along the other axis.
    let xs;
    let catStep;
    let xAxis = null;
    if (numericX) {
      const xVals = spec.labels;
      let x0 = Math.min(...xVals);
      let x1 = Math.max(...xVals);
      if (kind === 'scatter') { const p = (x1 - x0 || Math.abs(x0) || 1) * 0.04; x0 -= p; x1 += p; }
      xAxis = valueAxis(x0, x1, pad.left, pad.left + plotW, false, narrow ? 4 : 6, false);
      xs = xVals.map((x) => xAxis.scale(x));
      catStep = plotW / Math.max(1, n);
    } else if (horizontal) {
      catStep = plotH / n;
      xs = cats.map((_, i) => pad.top + (i + 0.5) * catStep);
    } else {
      catStep = plotW / n;
      xs = cats.map((_, i) => pad.left + (i + 0.5) * catStep);
    }

    const root = svg('svg', { viewBox: `0 0 ${width} ${height}`, width, height, class: `cedar-chart-svg${animate ? ' enter' : ''}` });
    // A CSS width too: WebKit (Safari, the macOS app) ignores the width attribute under the
    // stylesheet's `height: auto` and shrinks the plot to a speck.
    root.style.width = `${width}px`;
    root.setAttribute('role', 'img');
    root.setAttribute('aria-label', `${spec.title || KIND_NAMES[kind]}: ${data.map((s) => s.name).join(', ')}. Hover or use the arrow keys for values; the data is also in the table below.`);
    root.setAttribute('tabindex', '0');

    // Grid and axes.
    const grid = svg('g', { class: 'grid' });
    for (const t of axis.ticks) {
      const p = v(t);
      if (horizontal) {
        grid.append(svg('line', { x1: p, x2: p, y1: pad.top, y2: pad.top + plotH, class: t === 0 && !spec.log ? 'zero' : '' }));
        const label = svg('text', { x: p, y: pad.top + plotH + 16, class: 'axis', 'text-anchor': 'middle' });
        label.textContent = valueLabel(t);
        grid.append(label);
      } else {
        grid.append(svg('line', { x1: pad.left, x2: pad.left + plotW, y1: p, y2: p, class: t === 0 && !spec.log ? 'zero' : '' }));
        const label = svg('text', { x: pad.left - 8, y: p + 4, class: 'axis', 'text-anchor': 'end' });
        label.textContent = valueLabel(t);
        grid.append(label);
      }
    }
    if (xAxis) {
      for (const t of xAxis.ticks) {
        const label = svg('text', { x: xAxis.scale(t), y: pad.top + plotH + 16, class: 'axis', 'text-anchor': 'middle' });
        label.textContent = compact(t, xAxis.step);
        grid.append(label);
      }
    } else if (hist) {
      // Bin edges, as many as fit.
      const every = Math.max(1, Math.ceil((n + 1) / Math.max(2, Math.floor(plotW / 56))));
      for (let i = 0; i <= n; i += every) {
        const label = svg('text', { x: pad.left + i * catStep, y: pad.top + plotH + 16, class: 'axis', 'text-anchor': 'middle' });
        label.textContent = compact(hist.edges[i], (hist.edges[1] - hist.edges[0]) * every);
        grid.append(label);
      }
    } else if (horizontal) {
      const every = Math.max(1, Math.ceil(14 / catStep));
      cats.forEach((c, i) => {
        if (i % every) return;
        const label = svg('text', { x: pad.left - 8, y: xs[i] + 4, class: 'axis cat', 'text-anchor': 'end' });
        label.textContent = clip(labelText(c), catLabelWidth);
        grid.append(label);
      });
    } else {
      const longest = Math.max(...cats.map((c) => textWidth(labelText(c))));
      const every = Math.max(1, Math.ceil((Math.min(longest, 110) + 10) / catStep));
      cats.forEach((c, i) => {
        if (i % every) return;
        const label = svg('text', { x: xs[i], y: pad.top + plotH + 16, class: 'axis cat', 'text-anchor': 'middle' });
        label.textContent = clip(labelText(c), catStep * every - 6);
        grid.append(label);
      });
    }
    // The baseline the bars stand on.
    if (barLike) {
      const b = v(axis.base);
      grid.append(horizontal
        ? svg('line', { x1: b, x2: b, y1: pad.top, y2: pad.top + plotH, class: 'base' })
        : svg('line', { x1: pad.left, x2: pad.left + plotW, y1: b, y2: b, class: 'base' }));
    } else {
      grid.append(svg('line', { x1: pad.left, x2: pad.left + plotW, y1: pad.top + plotH, y2: pad.top + plotH, class: 'base' }));
    }
    // Axis titles, as matplotlib places them: x_label under the plot, y_label up its left side
    // (for barh, x is the value axis).
    if (spec.xLabel) {
      const t = svg('text', { x: pad.left + plotW / 2, y: height - 4, class: 'axis-title', 'text-anchor': 'middle' });
      t.textContent = clip(spec.xLabel, plotW);
      grid.append(t);
    }
    if (spec.yLabel) {
      const cy = pad.top + plotH / 2;
      const t = svg('text', { x: 12, y: cy, class: 'axis-title', 'text-anchor': 'middle', transform: `rotate(-90 12 ${cy})` });
      t.textContent = clip(spec.yLabel, plotH);
      grid.append(t);
    }
    root.append(grid);

    // The shaded band (a target range), behind the marks, with its label inside its top edge.
    if (target) {
      const a = v(target.from);
      const b = v(target.to);
      const g = svg('g', { class: 'target' });
      if (horizontal) {
        g.append(svg('rect', { x: Math.min(a, b), y: pad.top, width: Math.abs(b - a), height: plotH }));
        g.append(svg('line', { x1: a, x2: a, y1: pad.top, y2: pad.top + plotH }), svg('line', { x1: b, x2: b, y1: pad.top, y2: pad.top + plotH }));
      } else {
        g.append(svg('rect', { x: pad.left, y: Math.min(a, b), width: plotW, height: Math.abs(b - a) }));
        g.append(svg('line', { x1: pad.left, x2: pad.left + plotW, y1: a, y2: a }), svg('line', { x1: pad.left, x2: pad.left + plotW, y1: b, y2: b }));
      }
      if (target.label) {
        const t = horizontal
          ? svg('text', { x: Math.max(a, b) - 6, y: pad.top + 12, 'text-anchor': 'end' })
          : svg('text', { x: pad.left + plotW - 6, y: Math.min(a, b) + 13, 'text-anchor': 'end' });
        t.textContent = `${target.label} ${compact(target.from)}–${compact(target.to)}`;
        g.append(t);
      }
      root.append(g);
    }

    const band = svg('rect', { class: 'band' });
    band.style.display = 'none';
    root.append(band);
    const marks = svg('g', { class: 'marks' });
    root.append(marks);

    // The marks.
    const ends = [];
    if (barLike) {
      const inner = hist ? catStep - 2 : catStep * (data.length > 1 && !stacked ? 0.78 : 0.62);
      const groups = stacked ? 1 : data.length;
      const gap = groups > 1 ? 2 : 0;
      const thick = Math.max(1, (inner - gap * (groups - 1)) / groups);
      const pos = new Array(n).fill(axis.base);
      const neg = new Array(n).fill(axis.base);
      data.forEach((s, k) => {
        const g = svg('g', { class: slotClass(s.slot) });
        s.values.forEach((val, i) => {
          if (val === null || spec.log && val <= 0) return;
          let a;
          let b;
          if (stacked) {
            const from = val >= 0 ? pos[i] : neg[i];
            a = from; b = from + val;
            if (val >= 0) pos[i] = b; else neg[i] = b;
          } else { a = axis.base; b = val; }
          const start = xs[i] - inner / 2 + (stacked ? 0 : k * (thick + gap));
          const p0 = v(a);
          const p1 = v(b);
          // Only the outermost segment of a stack gets the rounded end.
          const outer = !stacked || data.slice(k + 1).every((o) => !((o.values[i] ?? 0) !== 0 && Math.sign(o.values[i]) === Math.sign(val)));
          let d;
          if (horizontal) {
            const x = Math.min(p0, p1);
            d = barPath(x, start, Math.abs(p1 - p0), thick, outer ? (b >= a ? 'right' : 'left') : '');
          } else {
            const y = Math.min(p0, p1);
            d = barPath(start, y, thick, Math.abs(p1 - p0), outer ? (b >= a ? 'top' : 'bottom') : '');
          }
          g.append(svg('path', { d, class: `bar${stacked ? ' seg' : ''}${b < a ? ' neg' : ''}` }));
        });
        marks.append(g);
      });
    } else if (kind === 'scatter') {
      for (const s of data) {
        const g = svg('g', { class: slotClass(s.slot) });
        s.values.forEach((val, i) => {
          if (val === null || spec.log && val <= 0) return;
          g.append(svg('circle', { cx: xs[i], cy: v(val), r: n > 400 ? 3 : 4.5, class: 'pt' }));
        });
        marks.append(g);
      }
    } else {
      // Lines and areas, in x order for a numeric x.
      const order = cats.map((_, i) => i);
      if (numericX) order.sort((a, b) => spec.labels[a] - spec.labels[b]);
      const base = new Array(n).fill(0);
      const drawn = [];
      for (const s of data) {
        const pts = order.map((i) => {
          const val = s.values[i];
          if (stacked) { const y = base[i] + (val ?? 0); base[i] = y; return [xs[i], v(y), i, base[i] - (val ?? 0)]; }
          if (val === null || spec.log && val <= 0) return null;
          return [xs[i], v(val), i];
        });
        drawn.push({ s, pts });
      }
      // Stacked areas are filled from the series below; plain areas from the baseline.
      drawn.forEach(({ s, pts }) => {
        const cls = slotClass(s.slot);
        if (kind === 'area') {
          let d = '';
          let run = [];
          const flush = () => {
            if (run.length > 1) {
              const top = run.map((p) => `${p[0].toFixed(1)},${p[1].toFixed(1)}`).join('L');
              const bottom = run.slice().reverse().map((p) => `${p[0].toFixed(1)},${(stacked ? v(p[3]) : v(axis.base)).toFixed(1)}`).join('L');
              d += `M${top}L${bottom}Z`;
            }
            run = [];
          };
          for (const p of pts) { if (p) run.push(p); else flush(); }
          flush();
          marks.append(svg('path', { d, class: `area ${cls}${stacked ? ' stacked' : ''}` }));
        }
      });
      drawn.forEach(({ s, pts }) => {
        const cls = slotClass(s.slot);
        const path = svg('path', { d: linePath(pts), class: `line ${cls}${animate ? ' draw' : ''}` });
        path.setAttribute('pathLength', '1');
        marks.append(path);
        const real = pts.filter(Boolean);
        if (kind === 'line' && real.length <= 40) {
          const g = svg('g', { class: cls });
          for (const p of real) g.append(svg('circle', { cx: p[0], cy: p[1], r: 3, class: 'pt small' }));
          marks.append(g);
        }
        if (real.length) ends.push({ s, p: real[real.length - 1] });
      });
    }

    // End labels name up to four lines in place, nudged apart so they don't overlap.
    if (endLabels && ends.length) {
      const labels = ends.map((e) => ({ e, y: e.p[1] })).sort((a, b) => a.y - b.y);
      for (let i = 1; i < labels.length; i++) labels[i].y = Math.max(labels[i].y, labels[i - 1].y + 14);
      const over = labels[labels.length - 1].y - (pad.top + plotH);
      if (over > 0) labels.forEach((l) => { l.y -= over; });
      for (const l of labels) {
        const t = svg('text', { x: pad.left + plotW + 8, y: l.y + 4, class: 'end-label' });
        t.textContent = clip(l.e.s.name, pad.right - 12);
        root.append(t);
      }
    }

    // -- hover -----------------------------------------------------------------------------
    const cross = svg('line', { class: 'cross' });
    cross.style.display = 'none';
    const dots = data.map((s) => {
      const c = svg('circle', { r: 5, class: `dot ${slotClass(s.slot)}` });
      c.style.display = 'none';
      return c;
    });
    root.append(cross, ...dots);
    const hit = svg('rect', { x: pad.left, y: pad.top, width: plotW, height: plotH, class: 'hit' });
    root.append(hit);

    const fmtCat = (i) => (hist ? cats[i] : labelText(spec.labels[i]));
    const catTitle = hist ? '' : horizontal ? spec.yLabel : spec.xLabel;
    /** Shows the tooltip right of `px` at height `py`, or left of `before` when it doesn't fit. */
    function place(px, py, before = px) {
      tip.hidden = false;
      const w = tip.offsetWidth;
      const h = tip.offsetHeight;
      let left = px + 14;
      if (left + w > width - 2) left = before - w - 14;
      tip.style.left = `${Math.max(2, left)}px`;
      tip.style.top = `${Math.max(2, Math.min(height - h - 2, py - h / 2))}px`;
    }
    function row(s, value, extra) {
      const r = el('div', 'row');
      r.append(el('span', `key ${slotClass(s.slot)}`), el('span', 'who', s.name), el('strong', '', value));
      if (extra) r.append(el('span', 'extra', extra));
      return r;
    }

    let cursor = -1;
    function showIndex(i) {
      cursor = Math.max(0, Math.min(n - 1, i));
      const at = xs[cursor];
      tip.replaceChildren(el('div', 'when', `${catTitle ? `${catTitle}: ` : ''}${fmtCat(cursor)}`));
      let total = 0;
      data.forEach((s) => {
        const val = s.values[cursor];
        total += val ?? 0;
        tip.append(row(s, full(val)));
      });
      if (stacked && data.length > 1) {
        const t = el('div', 'row total');
        t.append(el('span', 'who', 'Total'), el('strong', '', full(total)));
        tip.append(t);
      }
      if (barLike) {
        band.style.display = '';
        const bw = catStep;
        if (horizontal) Object.entries({ x: pad.left, y: at - bw / 2, width: plotW, height: bw }).forEach(([k, val]) => band.setAttribute(k, val));
        else Object.entries({ x: at - bw / 2, y: pad.top, width: bw, height: plotH }).forEach(([k, val]) => band.setAttribute(k, val));
        if (horizontal) place(pad.left + plotW * 0.6, at);
        else place(at + bw / 2 - 10, pad.top + plotH / 3, at - bw / 2 + 10);
      } else {
        cross.setAttribute('x1', at); cross.setAttribute('x2', at);
        cross.setAttribute('y1', pad.top); cross.setAttribute('y2', pad.top + plotH);
        cross.style.display = '';
        let acc = 0;
        data.forEach((s, k) => {
          const val = s.values[cursor];
          if (stacked) acc += val ?? 0;
          const y = stacked ? acc : val;
          if (y === null || spec.log && y <= 0) { dots[k].style.display = 'none'; return; }
          dots[k].setAttribute('cx', at);
          dots[k].setAttribute('cy', v(y));
          dots[k].style.display = '';
        });
        place(at, pad.top + plotH / 3);
      }
    }

    // A scatter's tooltip is the point nearest the pointer (and the arrow keys walk the points).
    const points = kind === 'scatter' ? data.flatMap((s, k) => s.values.map((val, i) => (val === null || spec.log && val <= 0 ? null : { s, k, i, x: xs[i], y: v(val) })).filter(Boolean)) : [];
    if (kind === 'scatter') points.sort((a, b) => a.x - b.x || a.y - b.y);
    function showPoint(j) {
      cursor = Math.max(0, Math.min(points.length - 1, j));
      const p = points[cursor];
      dots.forEach((d) => { d.style.display = 'none'; });
      dots[p.k].setAttribute('cx', p.x);
      dots[p.k].setAttribute('cy', p.y);
      dots[p.k].style.display = '';
      tip.replaceChildren(el('div', 'when', p.s.name));
      const r1 = el('div', 'row');
      r1.append(el('span', 'who', spec.xLabel || 'x'), el('strong', '', full(spec.labels[p.i])));
      const r2 = el('div', 'row');
      r2.append(el('span', 'who', spec.yLabel || 'y'), el('strong', '', full(p.s.values[p.i])));
      tip.append(r1, r2);
      place(p.x, p.y);
    }

    function hide() {
      cursor = -1;
      tip.hidden = true;
      cross.style.display = 'none';
      band.style.display = 'none';
      dots.forEach((d) => { d.style.display = 'none'; });
    }

    function pointer(e) {
      const box = root.getBoundingClientRect();
      const px = ((e.clientX - box.left) / box.width) * width;
      const py = ((e.clientY - box.top) / box.height) * height;
      if (kind === 'scatter') {
        let best = -1;
        let dist = 28 * 28;
        points.forEach((p, j) => { const d = (p.x - px) ** 2 + (p.y - py) ** 2; if (d < dist) { dist = d; best = j; } });
        if (best < 0) hide(); else showPoint(best);
        return;
      }
      const along = horizontal ? py : px;
      let best = 0;
      for (let i = 1; i < n; i++) if (Math.abs(xs[i] - along) < Math.abs(xs[best] - along)) best = i;
      showIndex(best);
    }
    hit.addEventListener('pointermove', pointer);
    hit.addEventListener('pointerdown', pointer);
    hit.addEventListener('pointerleave', hide);
    root.addEventListener('blur', hide);
    root.addEventListener('keydown', (e) => {
      const fwd = horizontal ? ['ArrowDown', 'ArrowRight'] : ['ArrowRight', 'ArrowUp'];
      const back = horizontal ? ['ArrowUp', 'ArrowLeft'] : ['ArrowLeft', 'ArrowDown'];
      let step = fwd.includes(e.key) ? 1 : back.includes(e.key) ? -1 : e.key === 'Home' ? -1e9 : e.key === 'End' ? 1e9 : 0;
      if (!step) { if (e.key === 'Escape') hide(); return; }
      e.preventDefault();
      if (kind === 'scatter') { if (points.length) showPoint(cursor < 0 ? 0 : cursor + step); return; }
      // Bars and lines in reading order: a numeric x walks left to right.
      if (numericX) {
        const order = cats.map((_, i) => i).sort((a, b) => xs[a] - xs[b]);
        const at = cursor < 0 ? -1 : order.indexOf(cursor);
        step = at < 0 ? (step > 0 ? 0 : order.length - 1) : Math.max(0, Math.min(order.length - 1, at + step));
        showIndex(order[step]);
        return;
      }
      showIndex(cursor < 0 ? 0 : cursor + step);
    });

    wrap.replaceChildren(root, tip);
  }

  // -- the pie ---------------------------------------------------------------------------------

  /** A donut and its list of slices (name, value, share), hovered together. */
  function drawPie(spec, wrap, tip, width, animate) {
    const values = spec.series[0].values;
    let slices = spec.labels.map((l, i) => ({ name: labelText(l), value: values[i] }))
      .filter((s) => s.value > 0)
      .map((s, i) => ({ ...s, slot: i }));
    if (slices.length > MAX_SLICES) {
      const sorted = [...slices].sort((a, b) => b.value - a.value);
      const kept = new Set(sorted.slice(0, MAX_SLICES - 1));
      const rest = slices.filter((s) => !kept.has(s));
      slices = slices.filter((s) => kept.has(s)).map((s, i) => ({ ...s, slot: i }));
      slices.push({ name: `Other (${rest.length})`, value: rest.reduce((a, s) => a + s.value, 0), slot: SLOTS });
    }
    const total = slices.reduce((a, s) => a + s.value, 0);
    const stack = width < 460;
    const size = Math.min(stack ? width - 8 : 240, 240);
    const r = size / 2 - 4;
    const inner = r * 0.62;
    const c = size / 2;

    const box = el('div', `cedar-chart-pie${stack ? ' stack' : ''}`);
    const root = svg('svg', { viewBox: `0 0 ${size} ${size}`, width: size, height: size, class: `cedar-chart-svg${animate ? ' enter' : ''}` });
    root.style.width = `${size}px`; // as in drawPlot, for WebKit
    root.setAttribute('role', 'img');
    root.setAttribute('aria-label', `${spec.title || 'Pie chart'}: ${slices.map((s) => `${s.name} ${((s.value / total) * 100).toFixed(1)}%`).join(', ')}`);
    root.setAttribute('tabindex', '0');
    const centerValue = svg('text', { x: c, y: c + 2, class: 'pie-total', 'text-anchor': 'middle' });
    const centerLabel = svg('text', { x: c, y: c + 20, class: 'pie-sub', 'text-anchor': 'middle' });
    const list = el('ol', 'cedar-chart-slices');
    const arcs = [];
    const rows = [];

    let a0 = -Math.PI / 2;
    for (const s of slices) {
      const share = s.value / total;
      const a1 = a0 + share * Math.PI * 2;
      const pt = (rad, a) => `${(c + rad * Math.cos(a)).toFixed(2)},${(c + rad * Math.sin(a)).toFixed(2)}`;
      let d;
      if (share >= 0.9999) {
        d = `M${pt(r, a0)}A${r},${r} 0 1 1 ${pt(r, a0 + Math.PI)}A${r},${r} 0 1 1 ${pt(r, a0)}M${pt(inner, a0)}A${inner},${inner} 0 1 0 ${pt(inner, a0 + Math.PI)}A${inner},${inner} 0 1 0 ${pt(inner, a0)}Z`;
      } else {
        const large = a1 - a0 > Math.PI ? 1 : 0;
        d = `M${pt(r, a0)}A${r},${r} 0 ${large} 1 ${pt(r, a1)}L${pt(inner, a1)}A${inner},${inner} 0 ${large} 0 ${pt(inner, a0)}Z`;
      }
      const arc = svg('path', { d, class: `slice ${slotClass(s.slot)}`, 'fill-rule': 'evenodd' });
      root.append(arc);
      arcs.push(arc);
      const li = el('li', slotClass(s.slot));
      li.append(el('span', 'swatch'), el('span', 'name', s.name), el('span', 'val', full(s.value)), el('span', 'pct', `${(share * 100).toFixed(1)}%`));
      list.append(li);
      rows.push(li);
      a0 = a1;
    }
    root.append(centerValue, centerLabel);

    let cursor = -1;
    function show(i) {
      cursor = (i + slices.length) % slices.length;
      arcs.forEach((a, k) => a.classList.toggle('dim', k !== cursor));
      rows.forEach((r2, k) => r2.classList.toggle('on', k === cursor));
      centerValue.textContent = `${((slices[cursor].value / total) * 100).toFixed(1)}%`;
      centerLabel.textContent = clip(slices[cursor].name, inner * 1.6, 11);
    }
    function reset() {
      cursor = -1;
      arcs.forEach((a) => a.classList.remove('dim'));
      rows.forEach((r2) => r2.classList.remove('on'));
      centerValue.textContent = compact(total, total / 100);
      centerLabel.textContent = 'Total';
    }
    reset();
    arcs.forEach((a, i) => {
      a.addEventListener('pointerenter', () => show(i));
      a.addEventListener('pointerdown', () => show(i));
    });
    rows.forEach((li, i) => li.addEventListener('pointerenter', () => show(i)));
    root.addEventListener('pointerleave', reset);
    list.addEventListener('pointerleave', reset);
    root.addEventListener('blur', reset);
    root.addEventListener('keydown', (e) => {
      const step = { ArrowRight: 1, ArrowDown: 1, ArrowLeft: -1, ArrowUp: -1 }[e.key];
      if (!step) { if (e.key === 'Escape') reset(); return; }
      e.preventDefault();
      show(cursor < 0 ? (step > 0 ? 0 : slices.length - 1) : cursor + step);
    });
    tip.hidden = true;
    box.append(root, list);
    wrap.replaceChildren(box);
  }

  // -- the table -------------------------------------------------------------------------------

  /** The data as a table, folded under a "Data" row that also carries `note`. */
  function table(spec, note) {
    const box = el('details', 'cedar-chart-data');
    const summary = el('summary');
    summary.append(el('span', 'label', 'Data'));
    if (note) summary.append(el('span', 'saved', note));
    box.append(summary);
    box.addEventListener('toggle', () => {
      if (!box.open || box.querySelector('table')) return;
      const tbl = el('table');
      const head = el('tr');
      let rows;
      if (spec.kind === 'histogram') {
        const hist = histogram(spec);
        head.append(el('th', '', spec.xLabel || 'Bin'), ...spec.series.map((s) => el('th', '', s.name)));
        rows = hist ? hist.counts[0].map((_, i) => [`${full(hist.edges[i])} – ${full(hist.edges[i + 1])}`, ...hist.counts.map((c) => c[i])]) : [];
      } else {
        head.append(el('th', '', (spec.kind === 'barh' ? spec.yLabel : spec.xLabel) || (spec.kind === 'pie' ? 'Slice' : '')), ...spec.series.map((s) => el('th', '', s.named || spec.series.length > 1 ? s.name : (spec.kind === 'barh' ? spec.xLabel : spec.yLabel) || 'Value')));
        rows = spec.labels.map((l, i) => [labelText(l), ...spec.series.map((s) => s.values[i])]);
        if (spec.kind === 'pie') {
          const total = spec.series[0].values.reduce((a, v) => a + (v || 0), 0);
          head.append(el('th', '', 'Share'));
          rows = rows.map((r) => [...r, `${((r[1] / total) * 100).toFixed(1)}%`]);
        }
      }
      const thead = el('thead');
      thead.append(head);
      const tbody = el('tbody');
      for (const r of rows.slice(0, MAX_TABLE_ROWS)) {
        const tr = el('tr');
        r.forEach((cell, k) => tr.append(el(k ? 'td' : 'th', '', typeof cell === 'number' || cell === null ? full(cell) : cell)));
        tbody.append(tr);
      }
      tbl.append(thead, tbody);
      const scroll = el('div', 'scroll');
      scroll.append(tbl);
      box.append(scroll);
      if (rows.length > MAX_TABLE_ROWS) box.append(el('p', 'note', `First ${MAX_TABLE_ROWS} of ${rows.length.toLocaleString()} rows; the saved file has them all.`));
    });
    return box;
  }

  // -- the card --------------------------------------------------------------------------------

  function element(spec) {
    const card = el('div', 'cedar-chart');
    card.setAttribute('role', 'group');
    card.setAttribute('aria-label', spec.title || KIND_NAMES[spec.kind]);

    const head = el('div', 'cedar-chart-head');
    const icon = el('span', 'cedar-chart-icon');
    icon.innerHTML = spec.kind === 'pie' ? PIE_ICON : ICON;
    const titles = el('div', 'titles');
    const title = el('div', 'cedar-chart-title', spec.title || KIND_NAMES[spec.kind]);
    title.setAttribute('role', 'heading');
    title.setAttribute('aria-level', '3');
    // The kind, unless it is already the title.
    const sub = spec.title ? [spec.stacked ? `Stacked ${KIND_NAMES[spec.kind].toLowerCase()}` : KIND_NAMES[spec.kind]] : [];
    if (spec.log) sub.push('log scale');
    if (spec.kind === 'histogram') sub.push(`${spec.series.reduce((a, s) => a + s.values.filter((v) => v !== null).length, 0).toLocaleString()} samples`);
    else if (spec.kind === 'pie') sub.push(`${spec.labels.length} slices`);
    else sub.push(`${spec.labels.length.toLocaleString()} ${spec.kind === 'scatter' ? 'points' : 'values'}${spec.series.length > 1 ? ` × ${spec.series.length} series` : ''}`);
    titles.append(title, el('div', 'cedar-chart-sub', sub.join(' · ')));
    head.append(icon, titles);
    card.append(head);

    // The legend: one entry per series, which hides or shows it. A single series needs none.
    const hidden = new Set();
    const legend = el('div', 'cedar-chart-legend');
    const wantsLegend = spec.kind !== 'pie' && (spec.series.length > 1 || spec.series[0].named);
    if (wantsLegend) {
      legend.setAttribute('role', 'group');
      legend.setAttribute('aria-label', 'Series');
      for (const s of spec.series) {
        const b = el('button', 'item');
        b.type = 'button';
        b.setAttribute('aria-pressed', 'true');
        b.title = spec.series.length > 1 ? `Show or hide ${s.name}` : s.name;
        b.append(el('span', `key ${slotClass(s.slot)}${spec.kind === 'line' || spec.kind === 'scatter' ? ' line' : ''}`), el('span', 'name', s.name));
        if (spec.series.length > 1) {
          b.addEventListener('click', () => {
            if (!hidden.has(s.slot) && hidden.size === spec.series.length - 1) return; // keep one
            if (hidden.has(s.slot)) hidden.delete(s.slot); else hidden.add(s.slot);
            b.setAttribute('aria-pressed', String(!hidden.has(s.slot)));
            draw(false);
          });
        } else b.disabled = true;
        legend.append(b);
      }
      card.append(legend);
    }

    const wrap = el('div', 'cedar-chart-plot');
    const tip = el('div', 'cedar-chart-tip');
    tip.hidden = true;
    card.append(wrap);

    let width = 0;
    let first = true;
    function draw(animate) {
      if (!width) return;
      if (spec.kind === 'pie') drawPie(spec, wrap, tip, width, animate);
      else drawPlot(spec, wrap, tip, width, hidden, animate);
    }
    // Drawn once the card is on the page and has a width, and again when that width changes.
    new ResizeObserver(() => {
      const w = Math.round(wrap.clientWidth);
      if (w && w !== width) { width = Math.max(240, w); draw(first); first = false; }
    }).observe(wrap);

    const foot = el('div', 'cedar-chart-foot');
    const notes = [];
    if (spec.thinned) notes.push('Long series are thinned here');
    if (spec.path) notes.push(`Saved as ${spec.path.split('/').pop()}`);
    foot.append(table(spec, notes.join(' · ')));
    card.append(foot);
    return card;
  }

  // -- charts in a tool's output --------------------------------------------------------------

  const CHART_LINE = /^\[chart\] (\{.*\})\s*$/;
  const MAX_OUTPUT_CHARTS = 6;
  const seen = new Map();

  /**
   * The charts a tool printed as `[chart] {spec}` lines (the spec is generate_chart's input, without
   * a path), and its output without them: `{charts: [spec], text}`. Any tool can draw this way; a
   * line that isn't a drawable spec is left in the text.
   */
  function fromOutput(output) {
    const key = String(output ?? '');
    if (seen.has(key)) return seen.get(key);
    const charts = [];
    const kept = [];
    for (const line of key.split('\n')) {
      const m = charts.length < MAX_OUTPUT_CHARTS && line.startsWith('[chart] ') ? CHART_LINE.exec(line) : null;
      let spec = null;
      if (m) {
        try { spec = parse('', { ...JSON.parse(m[1]), path: undefined }); } catch { spec = null; }
      }
      if (spec) charts.push(spec); else kept.push(line);
    }
    const found = { charts, text: charts.length ? kept.join('\n').trim() : key };
    if (seen.size > 50) seen.delete(seen.keys().next().value);
    seen.set(key, found);
    return found;
  }

  /** The rest of a tool's output, folded under its charts. */
  function report(text, label = 'Full report') {
    const box = el('details', 'cedar-chart-report');
    box.append(el('summary', '', label));
    box.append(el('pre', '', text || '(no other output)'));
    return box;
  }

  return { parse, element, fromOutput, report };
})();

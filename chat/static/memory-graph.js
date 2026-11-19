'use strict';

// Memory as a network, for Settings → Memory → Graph (see memory.js), shared by every UI. It
// draws cedar's `GET /v1/memory/graph`, `{nodes, edges, total, left_out}`: every memory a mark,
// and a line wherever two are linked by what cedar already stores (saved in the same
// conversation, sharing a topic, shown together to a run, one replacing the other).
//
//   - Colour and shape say which of the three lists a memory is in: about you (a circle), a
//     conversation (a square), a way of working (a diamond). Size is its rank, how much it
//     weighs when Cedar decides what to recall. A pinned memory wears a ring; an archived one is
//     hollow.
//   - The legend's entries hide and show their marks and lines.
//   - Pointing at a mark, or moving to it with Tab, lights up what it is linked to, names it and
//     shows its text; choosing it tells the page, which shows the memory with its actions.
//   - The picture is dragged to move it and zoomed with the buttons (or a pinch).
//
//   cedarMemoryGraph.element(graph, { selected, onSelect }) -> HTMLElement
//   cedarMemoryGraph.links(graph, id) -> [{ node, reasons: [string] }]   what a memory is linked to, and why
//
// The layout is worked out here, once per draw and the same every time for the same graph. It
// needs nothing from the app but `tr`. Memory text is only ever set as text.

const cedarMemoryGraph = (() => {
  const SVG = 'http://www.w3.org/2000/svg';
  const GROUPS = ['about', 'episodes', 'procedures'];
  const EDGE_KINDS = ['conversation', 'topic', 'recalled', 'superseded', 'merged'];
  const DIRECTED = ['superseded', 'merged'];
  // What the legend has switched off; kept while the page stays open, across redraws.
  const hidden = new Set();

  const groupOf = (node) => (node.kind === 'episode' ? 'episodes' : node.kind === 'procedure' ? 'procedures' : 'about');
  const isActive = (node) => !node.archived_at && !node.superseded_by;
  const radius = (node) => 4.5 + 5 * Math.min(Math.max(node.rank || 0, 0), 1.5);

  function el(tag, className, text) {
    const e = document.createElement(tag);
    if (className) e.className = className;
    if (text !== undefined) e.textContent = text;
    return e;
  }

  function svg(tag, attrs = {}) {
    const e = document.createElementNS(SVG, tag);
    for (const [k, v] of Object.entries(attrs)) e.setAttribute(k, String(v));
    return e;
  }

  const short = (text, max) => (text.length > max ? `${text.slice(0, max - 1).trimEnd()}…` : text);

  /** Where each node goes: a spring layout (Fruchterman and Reingold), from a fixed start. */
  function layout(nodes, edges) {
    const n = nodes.length;
    const index = new Map(nodes.map((node, i) => [node.id, i]));
    const pos = nodes.map((_, i) => {
      // A sunflower spiral: spread out, and the same for the same graph.
      const r = 26 * Math.sqrt(i + 0.5);
      const a = i * 2.399963;
      return { x: r * Math.cos(a), y: r * Math.sin(a) };
    });
    if (n < 2) return pos;
    const pairs = [];
    for (const e of edges) {
      const a = index.get(e.source);
      const b = index.get(e.target);
      if (a !== undefined && b !== undefined && a !== b) pairs.push([a, b, e.kind === 'topic' ? 0.6 : 1]);
    }
    const k = 40;
    const pull = Math.min(1.6, 0.4 + n / 50);
    const rounds = Math.round(Math.min(300, Math.max(60, 4e7 / (n * n))));
    const dx = new Float64Array(n);
    const dy = new Float64Array(n);
    for (let round = 0; round < rounds; round += 1) {
      dx.fill(0);
      dy.fill(0);
      for (let i = 0; i < n; i += 1) {
        for (let j = i + 1; j < n; j += 1) {
          let x = pos[i].x - pos[j].x;
          let y = pos[i].y - pos[j].y;
          let d2 = x * x + y * y;
          if (d2 < 0.01) { x = 0.1 + (i % 7) * 0.01; y = 0.1 + (j % 5) * 0.01; d2 = x * x + y * y; }
          const f = (k * k) / d2;
          dx[i] += x * f; dy[i] += y * f;
          dx[j] -= x * f; dy[j] -= y * f;
        }
      }
      for (const [a, b, strength] of pairs) {
        const x = pos[a].x - pos[b].x;
        const y = pos[a].y - pos[b].y;
        const f = (Math.sqrt(x * x + y * y) / k) * strength;
        dx[a] -= x * f; dy[a] -= y * f;
        dx[b] += x * f; dy[b] += y * f;
      }
      const heat = 30 * (1 - round / rounds) + 0.5;
      for (let i = 0; i < n; i += 1) {
        // A pull to the middle keeps the unlinked ones from drifting off: with it the whole
        // settles in a disc, with more room per memory when there are few.
        dx[i] -= pos[i].x * pull;
        dy[i] -= pos[i].y * pull;
        const d = Math.hypot(dx[i], dy[i]) || 1;
        const step = Math.min(d, heat);
        pos[i].x += (dx[i] / d) * step;
        pos[i].y += (dy[i] / d) * step;
      }
    }
    return pos;
  }

  /** Why two memories are linked, in words, as seen from `id`. */
  function reason(edge, id) {
    switch (edge.kind) {
      case 'topic': return tr('memory.graph.why.topic', { topics: (edge.topics || []).join(', ') });
      case 'recalled': return tr('memory.graph.why.recalled', { count: edge.weight });
      case 'superseded': return tr(edge.source === id ? 'memory.graph.why.replacedBy' : 'memory.graph.why.replaces');
      case 'merged': return tr(edge.source === id ? 'memory.graph.why.mergedInto' : 'memory.graph.why.absorbed');
      default: return tr('memory.graph.why.conversation');
    }
  }

  /** What the memory `id` is linked to, strongest first: `[{ node, reasons }]`. */
  function links(graph, id) {
    const byId = new Map(graph.nodes.map((node) => [node.id, node]));
    const found = new Map();
    for (const e of graph.edges) {
      if (e.source !== id && e.target !== id) continue;
      const other = byId.get(e.source === id ? e.target : e.source);
      if (!other) continue;
      if (!found.has(other.id)) found.set(other.id, { node: other, reasons: [], weight: 0 });
      const entry = found.get(other.id);
      entry.reasons.push(reason(e, id));
      entry.weight += e.weight;
    }
    return [...found.values()].sort((a, b) => b.weight - a.weight || b.node.rank - a.node.rank);
  }

  function shape(node, r) {
    const group = groupOf(node);
    if (group === 'about') return svg('circle', { r, class: 'mg-shape' });
    const side = r * 1.7;
    const rect = svg('rect', { x: -side / 2, y: -side / 2, width: side, height: side, rx: 1.5, class: 'mg-shape' });
    if (group === 'procedures') rect.setAttribute('transform', 'rotate(45)');
    return rect;
  }

  /** A legend mark like the one in the picture. */
  function swatch(group) {
    const s = svg('svg', { viewBox: '-8 -8 16 16', width: 14, height: 14, class: `mg-swatch g-${group}`, 'aria-hidden': 'true' });
    s.append(shape({ kind: group === 'episodes' ? 'episode' : group === 'procedures' ? 'procedure' : 'fact' }, group === 'about' ? 5.5 : 5));
    return s;
  }

  function lineSample(kind) {
    const s = svg('svg', { viewBox: '0 0 22 10', width: 22, height: 10, 'aria-hidden': 'true' });
    s.append(svg('line', { x1: 1, y1: 5, x2: DIRECTED.includes(kind) ? 15 : 21, y2: 5, class: `mg-edge k-${kind}` }));
    if (DIRECTED.includes(kind)) s.append(svg('path', { d: 'M14 1.5 L21 5 L14 8.5 Z', class: 'mg-arrow' }));
    return s;
  }

  function element(graph, opts = {}) {
    const { nodes, edges } = graph;
    const root = el('div', 'cedar-mgraph');
    const index = new Map(nodes.map((node, i) => [node.id, i]));
    const pos = layout(nodes, edges);
    let selected = index.has(opts.selected) ? opts.selected : null;

    // -- the legend: what the marks and lines mean, and switches for each --------------------
    const legend = el('div', 'cedar-mgraph-legend');
    const toggle = (key, label, sample) => {
      const b = el('button', 'cedar-mgraph-key');
      b.type = 'button';
      b.append(sample, el('span', '', label));
      b.setAttribute('aria-pressed', String(!hidden.has(key)));
      b.addEventListener('click', () => {
        if (hidden.has(key)) hidden.delete(key);
        else hidden.add(key);
        b.setAttribute('aria-pressed', String(!hidden.has(key)));
        filter();
      });
      return b;
    };
    const marks = el('div', 'cedar-mgraph-keys');
    for (const group of GROUPS) {
      const count = nodes.filter((node) => groupOf(node) === group).length;
      if (count) marks.append(toggle(`g:${group}`, `${tr(`memory.view.${group}`)} ${count}`, swatch(group)));
    }
    const lines = el('div', 'cedar-mgraph-keys');
    for (const kind of EDGE_KINDS) {
      if (edges.some((e) => e.kind === kind)) lines.append(toggle(`e:${kind}`, tr(`memory.graph.edge.${kind}`), lineSample(kind)));
    }
    legend.append(marks, lines);

    // -- the picture ---------------------------------------------------------------------------
    const stage = el('div', 'cedar-mgraph-stage');
    const pad = 28;
    const xs = pos.map((p) => p.x);
    const ys = pos.map((p) => p.y);
    const fit = {
      x: Math.min(...xs) - pad, y: Math.min(...ys) - pad,
      w: Math.max(Math.max(...xs) - Math.min(...xs) + 2 * pad, 560),
      h: Math.max(Math.max(...ys) - Math.min(...ys) + 2 * pad, 340),
    };
    // A handful of memories sit in the middle at their own size rather than fill the frame.
    fit.x -= (fit.w - (Math.max(...xs) - Math.min(...xs) + 2 * pad)) / 2;
    fit.y -= (fit.h - (Math.max(...ys) - Math.min(...ys) + 2 * pad)) / 2;
    let view = { ...fit };
    const picture = svg('svg', { class: 'cedar-mgraph-svg', role: 'group', preserveAspectRatio: 'xMidYMid meet' });
    picture.setAttribute('aria-label', tr('memory.graph.label', { nodes: nodes.length, edges: edges.length }));
    // How many of the picture's units one pixel is. Marks, arrowheads and names are drawn at
    // their own size however far the picture is zoomed out, and grow when it is zoomed in.
    let size = 1;
    let mark = 1;
    let lit = null;
    const setView = () => {
      picture.setAttribute('viewBox', `${view.x} ${view.y} ${view.w} ${view.h}`);
      const box = picture.getBoundingClientRect();
      size = box.width && box.height ? Math.max(1, view.w / box.width, view.h / box.height) : 1;
      // A crowded picture gets smaller marks.
      mark = size * (nodes.length > 150 ? 0.7 : 1);
      nodes.forEach((node, i) => nodeEls[i].setAttribute('transform', `translate(${pos[i].x} ${pos[i].y}) scale(${mark})`));
      edges.forEach((e, i) => {
        if (!arrowEls[i]) return;
        // Stop at the edge of the mark, where the arrowhead goes.
        const a = pos[index.get(e.source)];
        const b = pos[index.get(e.target)];
        const d = Math.hypot(b.x - a.x, b.y - a.y) || 1;
        const back = (radius(nodes[index.get(e.target)]) + 2) * mark;
        const x = b.x - ((b.x - a.x) / d) * back;
        const y = b.y - ((b.y - a.y) / d) * back;
        edgeEls[i].setAttribute('x2', x);
        edgeEls[i].setAttribute('y2', y);
        const turn = (Math.atan2(b.y - a.y, b.x - a.x) * 180) / Math.PI;
        arrowEls[i].setAttribute('transform', `translate(${x} ${y}) rotate(${turn}) scale(${mark})`);
      });
      light(lit);
    };

    const edgeLayer = svg('g');
    const nodeLayer = svg('g');
    const labelLayer = svg('g', { class: 'mg-labels', 'aria-hidden': 'true' });
    const edgeEls = edges.map((e) => {
      const a = pos[index.get(e.source)];
      const b = pos[index.get(e.target)];
      const line = svg('line', { x1: a.x, y1: a.y, x2: b.x, y2: b.y, class: `mg-edge k-${e.kind}` });
      line.style.strokeWidth = String(1 + Math.min(e.weight - 1, 3) * 0.6);
      edgeLayer.append(line);
      return line;
    });
    const arrowEls = edges.map((e) => {
      if (!DIRECTED.includes(e.kind)) return null;
      const head = svg('path', { d: 'M-6 -3 L0 0 L-6 3 Z', class: 'mg-arrow' });
      edgeLayer.append(head);
      return head;
    });
    const nodeEls = nodes.map((node, i) => {
      const r = radius(node);
      const g = svg('g', { class: `mg-node g-${groupOf(node)}`, transform: `translate(${pos[i].x} ${pos[i].y})`, tabindex: 0, role: 'button' });
      if (!isActive(node)) g.classList.add('inactive');
      g.setAttribute('aria-label', `${tr(`memory.kind.${node.kind}`)}: ${node.text}`);
      // A target bigger than the mark.
      g.append(svg('circle', { r: r + 6, class: 'mg-hit' }));
      if (node.pinned) g.append(svg('circle', { r: r + 3.5, class: 'mg-pin' }));
      g.append(shape(node, r));
      g.addEventListener('pointerenter', () => point(node.id));
      g.addEventListener('pointerleave', () => point(null));
      g.addEventListener('focus', () => point(node.id));
      g.addEventListener('blur', () => point(null));
      g.addEventListener('click', (ev) => { ev.stopPropagation(); select(node.id); });
      g.addEventListener('keydown', (ev) => {
        if (ev.key !== 'Enter' && ev.key !== ' ') return;
        ev.preventDefault();
        select(node.id);
      });
      nodeLayer.append(g);
      return g;
    });
    picture.append(edgeLayer, nodeLayer, labelLayer);

    const tip = el('div', 'cedar-mgraph-tip');
    tip.hidden = true;
    const zoom = el('div', 'cedar-mgraph-zoom');
    const zoomButton = (label, text, fn) => {
      const b = el('button', 'cedar-mgraph-zoom-btn', text);
      b.type = 'button';
      b.title = label;
      b.setAttribute('aria-label', label);
      b.addEventListener('click', fn);
      zoom.append(b);
    };
    const zoomBy = (factor, cx = view.x + view.w / 2, cy = view.y + view.h / 2) => {
      const w = Math.min(Math.max(view.w * factor, fit.w / 12), fit.w * 2.5);
      const f = w / view.w;
      view = { x: cx - (cx - view.x) * f, y: cy - (cy - view.y) * f, w, h: view.h * f };
      setView();
    };
    zoomButton(tr('memory.graph.zoomIn'), '+', () => zoomBy(1 / 1.4));
    zoomButton(tr('memory.graph.zoomOut'), '−', () => zoomBy(1.4));
    zoomButton(tr('memory.graph.fit'), '⤢', () => { view = { ...fit }; setView(); });
    stage.append(picture, zoom, tip);

    // -- what is shown, and what is lit ---------------------------------------------------------
    const shown = (node) => !hidden.has(`g:${groupOf(node)}`);
    const edgeShown = (e) => !hidden.has(`e:${e.kind}`) && shown(nodes[index.get(e.source)]) && shown(nodes[index.get(e.target)]);

    function filter() {
      nodes.forEach((node, i) => { nodeEls[i].style.display = shown(node) ? '' : 'none'; });
      edges.forEach((e, i) => {
        edgeEls[i].style.display = edgeShown(e) ? '' : 'none';
        if (arrowEls[i]) arrowEls[i].style.display = edgeEls[i].style.display;
      });
      light(selected);
    }

    /** Light up `id` and what it is linked to, with names; `null` lights everything evenly. */
    function light(id) {
      lit = id;
      labelLayer.replaceChildren();
      labelLayer.style.fontSize = `${10.5 * size}px`;
      labelLayer.style.strokeWidth = String(3 * size);
      const on = id !== null && index.has(id) && shown(nodes[index.get(id)]);
      picture.classList.toggle('lit', on);
      const near = new Set(on ? [id] : []);
      edges.forEach((e, i) => {
        const hot = on && edgeShown(e) && (e.source === id || e.target === id);
        edgeEls[i].classList.toggle('hot', hot);
        arrowEls[i]?.classList.toggle('hot', hot);
        if (hot) near.add(e.source === id ? e.target : e.source);
      });
      nodes.forEach((node, i) => {
        nodeEls[i].classList.toggle('hot', near.has(node.id));
        nodeEls[i].classList.toggle('sel', node.id === selected);
        // Names for the memory itself and a few of its neighbours; the list below has them all.
        if (!near.has(node.id) || (node.id !== id && labelLayer.childElementCount >= 7)) return;
        // To the right of the mark, unless that runs off the picture or over another mark and
        // the left is free.
        const free = (dir) => !nodes.some((other, j) => j !== i && shown(other)
          && Math.abs(pos[j].y - pos[i].y) < 9 * size
          && (pos[j].x - pos[i].x) * dir > 0 && Math.abs(pos[j].x - pos[i].x) < 120 * size);
        const fits = (dir) => (dir > 0 ? pos[i].x + 150 * size < view.x + view.w : pos[i].x - 150 * size > view.x);
        const side = (free(1) && fits(1)) || !(free(-1) && fits(-1)) ? 1 : -1;
        const label = svg('text', { x: pos[i].x + side * (radius(node) * mark + 5 * size), y: pos[i].y + 3.5 * size });
        if (side < 0) label.setAttribute('text-anchor', 'end');
        if (node.id === id) label.setAttribute('class', 'main');
        label.textContent = short(node.text, 26);
        labelLayer.append(label);
      });
    }

    function point(id) {
      light(id ?? selected);
      if (id === null) { tip.hidden = true; return; }
      const node = nodes[index.get(id)];
      const head = el('div', 'head');
      head.append(swatch(groupOf(node)), el('span', '', tr(`memory.kind.${node.kind}`)));
      if (node.pinned) head.append(el('span', '', `· ${tr('memory.pinned')}`));
      const parts = [head, el('div', 'text', short(node.text, 180))];
      if (node.topics?.length) parts.push(el('div', 'meta', node.topics.join(' · ')));
      tip.replaceChildren(...parts);
      tip.hidden = false;
      const box = stage.getBoundingClientRect();
      const at = nodeEls[index.get(id)].getBoundingClientRect();
      const left = Math.min(Math.max(at.left + at.width / 2 - box.left - tip.offsetWidth / 2, 6), box.width - tip.offsetWidth - 6);
      let top = at.top - box.top - tip.offsetHeight - 6;
      if (top < 6) top = at.bottom - box.top + 6;
      tip.style.left = `${left}px`;
      tip.style.top = `${top}px`;
    }

    function select(id) {
      selected = id;
      light(id);
      opts.onSelect?.(id);
    }

    // -- moving the picture ---------------------------------------------------------------------
    const scale = () => {
      const box = picture.getBoundingClientRect();
      return Math.max(view.w / (box.width || 1), view.h / (box.height || 1));
    };
    // The picture gets its size once it is on the page, and a new one when the window changes.
    if (typeof ResizeObserver !== 'undefined') new ResizeObserver(setView).observe(picture);
    let drag = null;
    picture.addEventListener('pointerdown', (ev) => {
      if (ev.target.closest('.mg-node') || ev.button !== 0) return;
      drag = { x: ev.clientX, y: ev.clientY, moved: false };
      picture.setPointerCapture(ev.pointerId);
    });
    picture.addEventListener('pointermove', (ev) => {
      if (!drag) return;
      const s = scale();
      const mx = ev.clientX - drag.x;
      const my = ev.clientY - drag.y;
      if (Math.abs(mx) + Math.abs(my) > 3) drag.moved = true;
      view.x -= mx * s;
      view.y -= my * s;
      drag.x = ev.clientX;
      drag.y = ev.clientY;
      setView();
    });
    const drop = (ev) => {
      if (!drag) return;
      // A click on the background lets go of the selection.
      if (!drag.moved && ev.type === 'pointerup' && selected !== null) select(null);
      drag = null;
    };
    picture.addEventListener('pointerup', drop);
    picture.addEventListener('pointercancel', drop);
    // A pinch (which arrives as a wheel with Ctrl) zooms; a plain wheel scrolls the page as usual.
    picture.addEventListener('wheel', (ev) => {
      if (!ev.ctrlKey && !ev.metaKey) return;
      ev.preventDefault();
      const box = picture.getBoundingClientRect();
      const s = scale();
      const cx = view.x + view.w / 2 + (ev.clientX - box.left - box.width / 2) * s;
      const cy = view.y + view.h / 2 + (ev.clientY - box.top - box.height / 2) * s;
      zoomBy(ev.deltaY > 0 ? 1.15 : 1 / 1.15, cx, cy);
    }, { passive: false });

    filter();
    setView();
    root.append(legend, stage);
    return root;
  }

  return { element, links, layout };
})();

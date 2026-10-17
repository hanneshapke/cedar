'use strict';

// Trip display for cedar's `plan_trip` tool, shared by both UIs (/v1 and /v2). The tool's result is a
// JSON object, {"title"?, "from", "to", "depart_at"? | "arrive_by"?, "recommendation": {"mode",
// "option", "reason"}, "options": [...], "unavailable"?: [{"mode", "reason"}], "attribution"}, and
// this turns it into a card: the recommendation, a map of the chosen option's route, the options to
// choose from, and the chosen one's steps (for transit, each walk and ride with its line).
//
//   cedarTransit.parse(output) -> spec | null   the validated result, or null if it isn't a usable trip
//   cedarTransit.element(spec) -> HTMLElement   the card; the map is drawn once the library has loaded
//
// It uses map-view.js (loaded first) for Leaflet and the base map. Everything in the result came from
// the model or the trip planner, so text is only ever set as text, colors must be #rrggbb, numbers
// are checked, and paths (encoded polylines, precision 5) are decoded with bounds checks.

const cedarTransit = (() => {
  const MAX_TEXT = 200;
  const MAX_OPTIONS = 8;
  const MAX_LEGS = 30;
  const MAX_POINTS = 2000;
  const MODES = new Set(['transit', 'walk', 'drive', 'rideshare']);
  const MODE_LABEL = { transit: 'Transit', walk: 'Walk', drive: 'Drive', rideshare: 'Rideshare' };
  // Leg modes as MOTIS names them (lowercased), grouped by the icon and word they get.
  const LEG_KIND = {
    walk: 'walk', bike: 'bike', rental: 'bike', car: 'drive', car_parking: 'drive', odm: 'drive', flex: 'bus',
    bus: 'bus', coach: 'bus', tram: 'tram', cable_car: 'tram', funicular: 'tram', areal_lift: 'tram',
    subway: 'subway', metro: 'subway', ferry: 'ferry', airplane: 'plane',
    rail: 'train', suburban: 'train', highspeed_rail: 'train', long_distance: 'train', night_rail: 'train',
    regional_rail: 'train', regional_fast_rail: 'train',
  };
  const KIND_LABEL = {
    walk: 'Walk', bike: 'Bike', drive: 'Drive', bus: 'Bus', tram: 'Tram', subway: 'Subway',
    ferry: 'Ferry', plane: 'Flight', train: 'Train', other: 'Ride',
  };
  // 24×24 icons, stroked with currentColor. Constant markup, never built from the result.
  const ICONS = {
    walk: 'M13 4.5a1.5 1.5 0 1 0 0-.01M10 21l2-6 3 3v3M9.5 11.5 11 8l3 2 2 3M11 8l-1 5',
    bike: 'M5.5 17.5a3 3 0 1 0 0-.01M18.5 17.5a3 3 0 1 0 0-.01M5.5 17.5 9 10h6l3.5 7.5M12 17.5 9 10M14 6h2l-1 4',
    drive: 'M5 16V12l2-5h10l2 5v4M4 16h16v2H4zM7 18v2M17 18v2M5 12h14M7.5 14h.01M16.5 14h.01',
    rideshare: 'M5 16V12l2-5h10l2 5v4M4 16h16v2H4zM7 18v2M17 18v2M5 12h14M10 4h4v3h-4z',
    bus: 'M6 4h12a1 1 0 0 1 1 1v12H5V5a1 1 0 0 1 1-1zM5 11h14M8 17v2M16 17v2M8 14h.01M16 14h.01',
    tram: 'M7 6h10v11H7zM7 12h10M9 17l-2 3M15 17l2 3M10 3h4M12 3v3',
    subway: 'M7 4h10a2 2 0 0 1 2 2v9a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2zM5 11h14M8 17l-2 3M16 17l2 3M9 14h.01M15 14h.01',
    train: 'M7 3h10a2 2 0 0 1 2 2v10a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2zM5 10h14M12 3v7M8 17l-2 4M16 17l2 4M9 14h.01M15 14h.01',
    ferry: 'M3 18c2 1.5 4 1.5 6 0s4-1.5 6 0 4 1.5 6 0M5 15l1-5h12l1 5M9 10V6h6v4M12 3v3',
    plane: 'M3 13l7-2 4-7h2l-2 7 6 1v2l-6 1 2 7h-2l-4-7-7-2z',
    other: 'M12 12m-7 0a7 7 0 1 0 14 0a7 7 0 1 0-14 0',
    transit: 'M7 3h10a2 2 0 0 1 2 2v10a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2zM5 10h14M12 3v7M8 17l-2 4M16 17l2 4M9 14h.01M15 14h.01',
  };

  function el(tag, className, text) {
    const e = document.createElement(tag);
    if (className) e.className = className;
    if (text !== undefined) e.textContent = text;
    return e;
  }

  function icon(kind) {
    const ns = 'http://www.w3.org/2000/svg';
    const svg = document.createElementNS(ns, 'svg');
    svg.setAttribute('viewBox', '0 0 24 24');
    svg.setAttribute('class', 'cedar-trip-icon');
    svg.setAttribute('aria-hidden', 'true');
    const path = document.createElementNS(ns, 'path');
    path.setAttribute('d', ICONS[kind] || ICONS.other);
    svg.append(path);
    return svg;
  }

  const str = (v) => (typeof v === 'string' && v.trim() ? v.trim().slice(0, MAX_TEXT) : '');
  const num = (v, max = 1e7) => (typeof v === 'number' && Number.isFinite(v) && v >= 0 && v <= max ? v : null);
  const coordinate = (v, limit) => typeof v === 'number' && Number.isFinite(v) && Math.abs(v) <= limit;
  const color = (v) => (typeof v === 'string' && /^#[0-9a-f]{6}$/i.test(v) ? v.toLowerCase() : '');
  const time = (v) => {
    const t = typeof v === 'string' ? Date.parse(v) : NaN;
    return Number.isFinite(t) ? new Date(t) : null;
  };

  /** Points from an encoded polyline with precision 5, or [] if it doesn't decode cleanly. */
  function decode(s) {
    if (typeof s !== 'string' || s.length > 20000) return [];
    const points = [];
    let i = 0; let lat = 0; let lon = 0;
    const next = () => {
      let result = 0; let shift = 0; let b;
      do {
        if (i >= s.length || shift > 30) return null;
        b = s.charCodeAt(i++) - 63;
        if (b < 0 || b > 63) return null;
        result |= (b & 0x1f) << shift;
        shift += 5;
      } while (b >= 0x20);
      return result & 1 ? ~(result >> 1) : result >> 1;
    };
    while (i < s.length && points.length < MAX_POINTS) {
      const dlat = next(); const dlon = next();
      if (dlat === null || dlon === null) return [];
      lat += dlat; lon += dlon;
      const p = [lat / 1e5, lon / 1e5];
      if (!coordinate(p[0], 90) || !coordinate(p[1], 180)) return [];
      points.push(p);
    }
    return points;
  }

  function place(p) {
    if (!p || !coordinate(p.latitude, 90) || !coordinate(p.longitude, 180)) return null;
    return { label: str(p.label) || `${p.latitude.toFixed(4)}, ${p.longitude.toFixed(4)}`, latitude: p.latitude, longitude: p.longitude };
  }

  function leg(l) {
    if (!l || typeof l !== 'object') return null;
    const mode = str(l.mode).toLowerCase();
    return {
      kind: LEG_KIND[mode] || 'other',
      from: str(l.from), to: str(l.to),
      depart: time(l.depart), arrive: time(l.arrive),
      minutes: num(l.duration_min) ?? 0,
      route: str(l.route).slice(0, 24), headsign: str(l.headsign), agency: str(l.agency),
      color: color(l.color), stops: num(l.stops, 1000), meters: num(l.distance_m),
      path: decode(l.path),
    };
  }

  function option(o) {
    if (!o || !MODES.has(o.mode)) return null;
    const minutes = num(o.duration_min);
    if (minutes === null) return null;
    const legs = o.mode === 'transit' && Array.isArray(o.legs) ? o.legs.slice(0, MAX_LEGS).map(leg).filter(Boolean) : [];
    const cost = o.cost && num(o.cost.low) !== null && num(o.cost.high) !== null
      ? { low: o.cost.low, high: o.cost.high, currency: str(o.cost.currency).slice(0, 3).toUpperCase() || 'USD' }
      : null;
    return {
      mode: o.mode, minutes, legs, cost,
      depart: time(o.depart), arrive: time(o.arrive),
      transfers: num(o.transfers, 50), walkMinutes: num(o.walk_min),
      km: num(o.distance_km, 1e5), meters: num(o.distance_m),
      note: str(o.note), path: decode(o.path),
    };
  }

  /** The trip described by a `plan_trip` result, or null if `output` isn't one with an option. */
  function parse(output) {
    let data;
    try { data = JSON.parse(output); } catch { return null; }
    if (!data || typeof data !== 'object' || !Array.isArray(data.options)) return null;
    const from = place(data.from); const to = place(data.to);
    if (!from || !to) return null;
    // Keep each option's index in the result, which the recommendation refers to.
    const options = [];
    data.options.slice(0, MAX_OPTIONS).forEach((o, index) => {
      const parsed = option(o);
      if (parsed) options.push({ ...parsed, index });
    });
    if (!options.length) return null;
    const rec = data.recommendation || {};
    const recommended = options.find((o) => o.index === rec.option) || null;
    const unavailable = (Array.isArray(data.unavailable) ? data.unavailable : [])
      .filter((u) => u && MODES.has(u.mode) && !options.some((o) => o.mode === u.mode))
      .slice(0, 4)
      .map((u) => ({ mode: u.mode, reason: str(u.reason) }));
    let timeZone;
    try {
      if (str(data.time_zone)) timeZone = new Intl.DateTimeFormat([], { timeZone: data.time_zone }).resolvedOptions().timeZone;
    } catch { /* an unknown zone: the viewer's is used */ }
    return {
      title: str(data.title), from, to, timeZone,
      departAt: time(data.depart_at), arriveBy: time(data.arrive_by),
      options, recommended, reason: recommended ? str(rec.reason) : '',
      unavailable, attribution: str(data.attribution),
    };
  }

  // -- formatting -----------------------------------------------------------------------------

  /** `d` as a time of day in the trip's time zone when the planner gave one, else the viewer's. */
  const clock = (spec, d, opts = {}) => d.toLocaleString([], { hour: 'numeric', minute: '2-digit', timeZone: spec.timeZone, ...opts });
  function duration(min) {
    if (min < 60) return `${min} min`;
    const h = Math.floor(min / 60); const m = min % 60;
    return m ? `${h} h ${m} min` : `${h} h`;
  }
  const distance = (o) => (o.km !== null ? `${o.km} km` : o.meters !== null ? (o.meters >= 1000 ? `${(o.meters / 1000).toFixed(1)} km` : `${Math.round(o.meters)} m`) : '');
  function money(cost) {
    const sym = cost.currency === 'USD' ? '$' : cost.currency === 'EUR' ? '€' : cost.currency === 'GBP' ? '£' : `${cost.currency} `;
    return cost.low === cost.high ? `~${sym}${cost.low}` : `~${sym}${cost.low}–${cost.high}`;
  }
  /** Black or white, whichever reads better on `hex`. */
  function ink(hex) {
    const [r, g, b] = [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16) / 255);
    return 0.2126 * r + 0.7152 * g + 0.0722 * b > 0.55 ? '#111' : '#fff';
  }

  /** "U5" on its line color, or the leg's kind when it has no line name. */
  function badge(l) {
    const b = el('span', 'cedar-trip-line');
    b.append(icon(l.kind));
    if (l.route) b.append(el('span', '', l.route));
    if (l.color) { b.style.background = l.color; b.style.color = ink(l.color); b.classList.add('colored'); }
    return b;
  }

  /** The facts under an option's name: its times, transfers, distance or price. */
  function facts(spec, o) {
    const out = [];
    if (o.depart && o.arrive) out.push(`${clock(spec, o.depart)} – ${clock(spec, o.arrive)}`);
    if (o.mode === 'transit') {
      if (o.transfers !== null) out.push(o.transfers === 0 ? 'direct' : `${o.transfers} transfer${o.transfers === 1 ? '' : 's'}`);
      if (o.walkMinutes) out.push(`${o.walkMinutes} min walk`);
    } else {
      const d = distance(o);
      if (d) out.push(d);
    }
    if (o.cost) out.push(money(o.cost));
    return out.join(' · ');
  }

  // -- the map ----------------------------------------------------------------------------------

  /** The lines to draw for `o`: each transit leg in its color, walks dashed, a car route solid. */
  function lines(spec, o) {
    if (o.mode === 'transit') {
      return o.legs.filter((l) => l.path.length > 1).map((l) => ({
        points: l.path, walk: l.kind === 'walk', color: l.kind === 'walk' ? '' : l.color,
      }));
    }
    // A rideshare drives the car's route.
    const path = o.path.length > 1 ? o.path : (spec.options.find((x) => x.mode === 'drive')?.path || []);
    return path.length > 1 ? [{ points: path, walk: o.mode === 'walk', color: '' }] : [];
  }

  function endPin(L, text, cls) {
    const body = el('span', `cedar-pin ${cls}`);
    body.append(el('span', 'cedar-pin-n', text));
    return L.divIcon({ html: body, className: 'cedar-pin-icon', iconSize: [30, 38], iconAnchor: [15, 37] });
  }

  function draw(L, canvas, spec, tilesFailed) {
    const map = L.map(canvas, { scrollWheelZoom: false });
    map.setView([spec.from.latitude, spec.from.longitude], 12);
    cedarMap.tiles(L).addTo(map).once('tileerror', tilesFailed);
    const accent = getComputedStyle(canvas).getPropertyValue('--accent').trim() || '#c96442';
    const routes = L.layerGroup().addTo(map);
    const a = [spec.from.latitude, spec.from.longitude]; const b = [spec.to.latitude, spec.to.longitude];
    L.marker(a, { icon: endPin(L, 'A', 'start'), title: spec.from.label, alt: spec.from.label }).addTo(map);
    L.marker(b, { icon: endPin(L, 'B', 'end'), title: spec.to.label, alt: spec.to.label }).addTo(map);
    map.on('click', () => map.scrollWheelZoom.enable());
    map.on('mouseout', () => map.scrollWheelZoom.disable());

    let shown = null;
    const show = (o) => {
      shown = o;
      routes.clearLayers();
      const drawn = lines(spec, o);
      if (!drawn.length) {
        // No geometry: a dashed line still says where the trip goes.
        L.polyline([a, b], { color: accent, weight: 3, opacity: 0.7, dashArray: '2 8' }).addTo(routes);
      }
      for (const line of drawn) {
        const c = line.walk ? '#6f6d66' : line.color || accent;
        // A white casing under each line keeps it legible on any tile.
        L.polyline(line.points, { color: '#fff', weight: line.walk ? 6 : 8, opacity: 0.9 }).addTo(routes);
        L.polyline(line.points, { color: c, weight: line.walk ? 3 : 5, opacity: 1, dashArray: line.walk ? '1 7' : null, lineCap: 'round' }).addTo(routes);
      }
      const all = [a, b, ...drawn.flatMap((l) => l.points)];
      // Room above for the pins and on the left for the zoom buttons.
      if (canvas.clientWidth && canvas.clientHeight) {
        map.fitBounds(L.latLngBounds(all), { paddingTopLeft: [56, 48], paddingBottomRight: [28, 24], maxZoom: 16 });
      }
    };
    // The card may be built before it is on the page (and the desk moves it), so the view is fitted
    // when the map first has a size, and the map is told whenever that size changes.
    let fitted = false;
    new ResizeObserver(() => {
      if (!canvas.clientWidth || !canvas.clientHeight) return;
      map.invalidateSize();
      if (!fitted && shown) { fitted = true; show(shown); }
    }).observe(canvas);
    return show;
  }

  // -- the card ---------------------------------------------------------------------------------

  /** The chosen option's steps: for transit each leg, otherwise one line with the totals. */
  function steps(spec, o) {
    const list = el('ol', 'cedar-trip-steps');
    const step = (kind, lineEl, main, sub, at) => {
      const li = el('li', `cedar-trip-step ${kind}`);
      li.append(el('span', 'cedar-trip-time', at ? clock(spec, at) : ''));
      const dot = el('span', 'cedar-trip-dot');
      li.append(dot);
      const body = el('div', 'cedar-trip-step-body');
      const head = el('div', 'cedar-trip-step-head');
      if (lineEl) head.append(lineEl);
      head.append(el('span', '', main));
      body.append(head);
      if (sub) body.append(el('div', 'cedar-trip-step-sub', sub));
      li.append(body);
      return li;
    };
    if (o.mode !== 'transit' || !o.legs.length) {
      const verb = { walk: 'Walk', drive: 'Drive', rideshare: 'Ride' }[o.mode] || 'Go';
      list.append(step(o.mode, null, `${verb} to ${spec.to.label}`, [duration(o.minutes), distance(o), o.note].filter(Boolean).join(' · '), o.depart));
    } else {
      o.legs.forEach((l, i) => {
        const to = l.to && l.to !== 'END' ? l.to : spec.to.label;
        if (l.kind === 'walk') {
          const far = l.meters !== null ? ` · ${l.meters >= 1000 ? `${(l.meters / 1000).toFixed(1)} km` : `${l.meters} m`}` : '';
          list.append(step('walk', null, `Walk to ${to}`, `${duration(l.minutes)}${far}`, l.depart));
          return;
        }
        const from = l.from && l.from !== 'START' ? l.from : spec.from.label;
        const towards = l.headsign ? ` towards ${l.headsign}` : '';
        const lineEl = badge(l);
        const li = step('ride', lineEl, `${KIND_LABEL[l.kind]}${towards}`,
          [`${from} → ${to}`, l.stops ? `${l.stops} stop${l.stops === 1 ? '' : 's'}` : '', duration(l.minutes), l.agency].filter(Boolean).join(' · '),
          l.depart);
        if (l.color) li.style.setProperty('--leg', l.color);
        list.append(li);
        if (i === o.legs.length - 1 && l.arrive) list.append(step('end', null, `Arrive at ${to}`, '', l.arrive));
      });
    }
    const last = o.legs[o.legs.length - 1];
    if ((o.mode !== 'transit' || last?.kind === 'walk') && o.arrive) list.append(step('end', null, `Arrive at ${spec.to.label}`, '', o.arrive));
    return list;
  }

  function optionRow(spec, o, fastest, select) {
    const row = el('button', `cedar-trip-option mode-${o.mode}`);
    row.type = 'button';
    const ico = el('span', 'cedar-trip-mode');
    ico.append(icon(o.mode));
    const main = el('span', 'cedar-trip-option-main');
    const name = el('span', 'cedar-trip-option-name');
    name.append(el('span', '', MODE_LABEL[o.mode]));
    if (o === spec.recommended) name.append(el('span', 'cedar-trip-tag rec', 'Recommended'));
    if (o === fastest && o !== spec.recommended) name.append(el('span', 'cedar-trip-tag', 'Fastest'));
    main.append(name);
    if (o.mode === 'transit' && o.legs.some((l) => l.kind !== 'walk')) {
      const chain = el('span', 'cedar-trip-chain');
      o.legs.filter((l) => l.kind !== 'walk').slice(0, 5).forEach((l, i) => {
        if (i) chain.append(el('span', 'cedar-trip-chev', '›'));
        chain.append(badge(l));
      });
      main.append(chain);
    }
    const f = facts(spec, o);
    if (f) main.append(el('span', 'cedar-trip-facts', f));
    row.append(ico, main, el('span', 'cedar-trip-duration', duration(o.minutes)));
    row.addEventListener('click', () => select(o));
    return row;
  }

  /** The card for `spec`. */
  function element(spec) {
    const card = el('div', 'cedar-trip');
    card.setAttribute('role', 'group');
    const route = `${spec.from.label} → ${spec.to.label}`;
    card.setAttribute('aria-label', spec.title || `Trip from ${spec.from.label} to ${spec.to.label}`);

    const head = el('div', 'cedar-trip-head');
    head.append(el('div', 'cedar-trip-title', spec.title || route));
    const when = spec.arriveBy ? `Arrive by ${clock(spec, spec.arriveBy, { weekday: 'short' })}`
      : spec.departAt ? `Leave ${clock(spec, spec.departAt, { weekday: 'short' })}` : 'Leaving now';
    head.append(el('div', 'cedar-trip-sub', spec.title ? `${route} · ${when}` : when));
    card.append(head);

    if (spec.recommended) {
      const rec = el('div', 'cedar-trip-rec');
      const ico = el('span', 'cedar-trip-mode');
      ico.append(icon(spec.recommended.mode));
      const text = el('div', '');
      text.append(el('strong', '', `Take ${MODE_LABEL[spec.recommended.mode].toLowerCase()}: ${duration(spec.recommended.minutes)}`));
      if (spec.reason) text.append(el('div', 'cedar-trip-reason', spec.reason));
      rec.append(ico, text);
      card.append(rec);
    }

    const frame = el('div', 'cedar-map-frame cedar-trip-frame');
    const canvas = el('div', 'cedar-map-canvas');
    const status = el('div', 'cedar-map-status', 'Loading map…');
    frame.append(canvas, status);
    const note = el('div', 'cedar-map-note');
    note.hidden = true;
    card.append(frame, note);

    const fastest = spec.options.reduce((best, o) => (o.minutes < best.minutes ? o : best));
    const list = el('div', 'cedar-trip-options');
    list.setAttribute('role', 'list');
    const detail = el('div', 'cedar-trip-detail');
    card.append(list, detail);

    let showOnMap = null;
    let current = null;
    const rows = new Map();
    const select = (o) => {
      current = o;
      for (const [opt, row] of rows) row.setAttribute('aria-pressed', String(opt === o));
      detail.replaceChildren(steps(spec, o));
      showOnMap?.(o);
    };
    for (const o of spec.options) {
      const row = optionRow(spec, o, fastest, select);
      rows.set(o, row);
      list.append(row);
    }
    select(spec.recommended || spec.options[0]);

    const foot = el('div', 'cedar-trip-foot');
    for (const u of spec.unavailable) {
      foot.append(el('div', '', `${MODE_LABEL[u.mode]}: ${u.reason || 'not available'}`));
    }
    if (spec.attribution) foot.append(el('div', 'cedar-trip-attr', spec.attribution));
    if (foot.childElementCount) card.append(foot);

    cedarMap.loadLeaflet()
      .then((L) => {
        status.remove();
        showOnMap = draw(L, canvas, spec, () => {
          note.textContent = 'The map tiles could not be loaded. The options are listed below.';
          note.hidden = false;
        });
        showOnMap(current);
      })
      .catch((e) => {
        status.textContent = e?.message || 'Could not draw the map.';
        status.classList.add('failed');
      });
    return card;
  }

  return { parse, element };
})();

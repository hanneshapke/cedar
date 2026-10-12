'use strict';

// Map display for cedar's `show_map` tool, shared by both UIs (/v1 and /v2). The tool's result is a
// JSON object, {"title"?, "markers": [{"label", "latitude", "longitude"}], "not_found"?: [names]},
// and this turns it into a card with a map, numbered pins, and a list of the places:
//
//   cedarMap.parse(output) -> spec | null   the validated result, or null if it isn't a usable map
//   cedarMap.element(spec) -> HTMLElement   the card; the map is drawn once the library has loaded
//
// It needs nothing from the app. The map library (Leaflet, vendored under /vendor/leaflet and served
// by this server) is only loaded when the first map is shown. Everything in the result came from the
// model, so text is only ever set as text, and coordinates are checked before they are used.

const cedarMap = (() => {
  const LEAFLET_JS = '/vendor/leaflet/leaflet.js';
  const LEAFLET_CSS = '/vendor/leaflet/leaflet.css';
  const TILES = 'https://tile.openstreetmap.org/{z}/{x}/{y}.png';
  const ATTRIBUTION = '&copy; <a href="https://www.openstreetmap.org/copyright" target="_blank" ' +
    'rel="noopener noreferrer">OpenStreetMap</a> contributors';
  const MAX_MARKERS = 25;
  const MAX_TEXT = 200;
  const SINGLE_PIN_ZOOM = 10;

  let library = null; // the pending or finished load of Leaflet

  function el(tag, className, text) {
    const e = document.createElement(tag);
    if (className) e.className = className;
    if (text !== undefined) e.textContent = text;
    return e;
  }

  const coordinate = (v, limit) => typeof v === 'number' && Number.isFinite(v) && Math.abs(v) <= limit;
  const clip = (s) => s.trim().slice(0, MAX_TEXT);

  /** The map described by a `show_map` result, or null if `output` isn't one with a valid pin. */
  function parse(output) {
    let data;
    try { data = JSON.parse(output); } catch { return null; }
    if (!data || typeof data !== 'object' || !Array.isArray(data.markers)) return null;
    const markers = [];
    for (const m of data.markers.slice(0, MAX_MARKERS)) {
      if (!m || !coordinate(m.latitude, 90) || !coordinate(m.longitude, 180)) continue;
      const label = typeof m.label === 'string' && m.label.trim()
        ? clip(m.label)
        : `${m.latitude.toFixed(4)}, ${m.longitude.toFixed(4)}`;
      markers.push({ label, latitude: m.latitude, longitude: m.longitude });
    }
    if (!markers.length) return null;
    const notFound = Array.isArray(data.not_found)
      ? data.not_found.filter((n) => typeof n === 'string' && n.trim()).slice(0, MAX_MARKERS).map(clip)
      : [];
    const title = typeof data.title === 'string' && data.title.trim() ? clip(data.title) : '';
    return { title, markers, notFound };
  }

  function loadStyle(href) {
    return new Promise((resolve, reject) => {
      const link = el('link');
      link.rel = 'stylesheet';
      link.href = href;
      link.onload = resolve;
      link.onerror = () => reject(new Error('Could not load the map library.'));
      document.head.append(link);
    });
  }

  function loadScript(src) {
    return new Promise((resolve, reject) => {
      const script = el('script');
      script.src = src;
      script.onload = resolve;
      script.onerror = () => reject(new Error('Could not load the map library.'));
      document.head.append(script);
    });
  }

  /** Leaflet, loaded on first use. A failed load is forgotten so the next map can try again. */
  function loadLeaflet() {
    library ??= Promise.all([loadStyle(LEAFLET_CSS), loadScript(LEAFLET_JS)])
      .then(() => window.L)
      .catch((e) => { library = null; throw e; });
    return library;
  }

  /** A numbered pin. Its number is the one in the list under the map. */
  function pin(L, n) {
    const body = el('span', 'cedar-pin');
    body.append(el('span', 'cedar-pin-n', String(n)));
    return L.divIcon({
      html: body, className: 'cedar-pin-icon', iconSize: [30, 38], iconAnchor: [15, 37], popupAnchor: [0, -34],
    });
  }

  function popup(marker) {
    const box = el('div', 'cedar-map-popup');
    box.append(el('strong', '', marker.label), el('div', 'cedar-map-coords', coords(marker)));
    return box;
  }

  const coords = (m) => `${m.latitude.toFixed(4)}, ${m.longitude.toFixed(4)}`;

  function draw(L, canvas, spec, tilesFailed) {
    const map = L.map(canvas, { scrollWheelZoom: false, worldCopyJump: true });
    map.setView([0, 0], 1);
    const tiles = L.tileLayer(TILES, { maxZoom: 19, attribution: ATTRIBUTION }).addTo(map);
    tiles.once('tileerror', tilesFailed);
    spec.markers.forEach((m, i) => {
      L.marker([m.latitude, m.longitude], { icon: pin(L, i + 1), title: m.label, alt: m.label })
        .bindPopup(popup(m))
        .addTo(map);
    });
    // The wheel scrolls the conversation until the map is clicked.
    map.on('click', () => map.scrollWheelZoom.enable());
    map.on('mouseout', () => map.scrollWheelZoom.disable());

    const points = spec.markers.map((m) => [m.latitude, m.longitude]);
    const fit = () => {
      if (points.length === 1) map.setView(points[0], SINGLE_PIN_ZOOM);
      else map.fitBounds(L.latLngBounds(points), { padding: [32, 32], maxZoom: 14 });
    };
    // The card may be built before it is on the page (and the desk moves it), so the view is fitted
    // when the map first has a size, and the map is told whenever that size changes.
    let fitted = false;
    new ResizeObserver(() => {
      if (!canvas.clientWidth || !canvas.clientHeight) return;
      map.invalidateSize();
      if (!fitted) { fitted = true; fit(); }
    }).observe(canvas);
  }

  /** The card for `spec`: title, map, a note if tiles fail, the places, and any names not found. */
  function element(spec) {
    const card = el('div', 'cedar-map');
    card.setAttribute('role', 'group');
    card.setAttribute('aria-label', spec.title || `Map of ${spec.markers.length} place${spec.markers.length === 1 ? '' : 's'}`);
    if (spec.title) card.append(el('div', 'cedar-map-title', spec.title));

    const frame = el('div', 'cedar-map-frame');
    const canvas = el('div', 'cedar-map-canvas');
    const status = el('div', 'cedar-map-status', 'Loading map…');
    frame.append(canvas, status);
    const note = el('div', 'cedar-map-note');
    note.hidden = true;
    card.append(frame, note);

    const list = el('ol', 'cedar-map-legend');
    spec.markers.forEach((m, i) => {
      const item = el('li');
      item.append(el('span', 'cedar-map-badge', String(i + 1)), el('span', 'cedar-map-label', m.label), el('span', 'cedar-map-coords', coords(m)));
      list.append(item);
    });
    card.append(list);
    if (spec.notFound.length) card.append(el('div', 'cedar-map-note', `Not found: ${spec.notFound.join(', ')}`));

    loadLeaflet()
      .then((L) => {
        status.remove();
        draw(L, canvas, spec, () => {
          note.textContent = 'The map tiles could not be loaded. The places are listed below.';
          note.hidden = false;
        });
      })
      .catch((e) => {
        status.textContent = e?.message || 'Could not draw the map.';
        status.classList.add('failed');
      });
    return card;
  }

  return { parse, element };
})();

'use strict';

// Flights display for cedar's `search_flights` tool, shared by every UI. The tool's result is a
// JSON object of one of two kinds (see src/tools/flights.rs):
//
//   {"kind": "flights", "leg": "one_way" | "outbound" | "return", "from": [code], "to": [code],
//    "date", "return_date"?, "passengers": {"adults", "children", "infants_in_seat",
//    "infants_on_lap"}, "cabin", "currency", "sort", "options": [option], "more", "no_results"?,
//    "insights"?: {"lowest", "level"?, "typical"?: [low, high], "history"?: [{"date", "price"}]},
//    "google_flights_url"?, "as_of", "note"}
//   {"kind": "booking", ...the same header, "flights": [option], "options": [{"seller", "part",
//    "price", "fare"?, "bags", "marketed_as"}], "no_results"?, "google_flights_url"?, "note"}
//
// where an option is {"id", "top_pick", "price", "airlines", "logo"?, "depart", "arrive",
// "duration_min"?, "stops", "segments": [...], "layovers": [...], "co2_kg"?, "co2_vs_typical_pct"?}.
// This turns it into a card: the route, day, passengers and cabin; Google's price insight with a
// sparkline of the price history; a row per flight (price, airlines, times, duration, stops) that
// opens to its segments and layovers; or, for a booking, the chosen flights and where to book them.
//
//   cedarFlights.parse(output) -> spec | null   the validated result, or null if it isn't one
//   cedarFlights.element(spec) -> HTMLElement   the card
//
// It needs nothing from the app. Everything in the result came from Google Flights, so text is only
// ever set as text, numbers are checked, and logos and links are only used if they are https URLs.
// Times are the airports' local times without a zone: they are shown as given, never converted.

const cedarFlights = (() => {
  const MAX_TEXT = 200;
  const MAX_OPTIONS = 30;
  const MAX_SEGMENTS = 10;
  const MAX_SELLERS = 20;
  const MAX_HISTORY = 120;
  const CABINS = {
    economy: 'flights.cabin.economy', premium_economy: 'flights.cabin.premiumEconomy',
    business: 'flights.cabin.business', first: 'flights.cabin.first',
  };
  const LEVELS = { low: 'flights.level.low', typical: 'flights.level.typical', high: 'flights.level.high' };
  const PARTS = { departing: 'flights.part.departing', returning: 'flights.part.returning' };
  const PASSENGERS = [
    ['adults', 'flights.pax.adults'], ['children', 'flights.pax.children'],
    ['infants_in_seat', 'flights.pax.infantsInSeat'], ['infants_on_lap', 'flights.pax.infantsOnLap'],
  ];
  // 24×24 icons, stroked with currentColor. Constant markup, never built from the result.
  const ICONS = {
    plane: 'M12 2c.8 0 1.5.7 1.5 1.5V9l7.5 4.5V16l-7.5-2.5V18l2 1.5V21L12 20l-3.5 1v-1.5l2-1.5v-4.5L3 16v-2.5L10.5 9V3.5C10.5 2.7 11.2 2 12 2z',
    chevron: 'M9 6l6 6-6 6',
    out: 'M14 5h5v5M19 5l-8 8M19 14v5H5V5h5',
    clock: 'M12 12m-8 0a8 8 0 1 0 16 0a8 8 0 1 0-16 0M12 8v4l3 2',
    moon: 'M19 14.5A7.5 7.5 0 0 1 9.5 5a7.5 7.5 0 1 0 9.5 9.5z',
    leaf: 'M5 19c0-8 5-13 14-14-1 9-6 14-14 14zM5 19l7-7',
    star: 'M12 3.5l2.6 5.6 6.1.7-4.5 4.2 1.2 6-5.4-3-5.4 3 1.2-6L3.3 9.8l6.1-.7z',
    bag: 'M6 8h12l-1 12H7zM9 8V6a3 3 0 0 1 6 0v2',
  };

  function el(tag, className, text) {
    const e = document.createElement(tag);
    if (className) e.className = className;
    if (text !== undefined) e.textContent = text;
    return e;
  }

  function icon(name, className = 'cedar-flights-icon') {
    const ns = 'http://www.w3.org/2000/svg';
    const svg = document.createElementNS(ns, 'svg');
    svg.setAttribute('viewBox', '0 0 24 24');
    svg.setAttribute('class', className);
    svg.setAttribute('aria-hidden', 'true');
    const path = document.createElementNS(ns, 'path');
    path.setAttribute('d', ICONS[name]);
    svg.append(path);
    return svg;
  }

  // -- validating the result ------------------------------------------------------------------

  const str = (v, max = MAX_TEXT) => (typeof v === 'string' && v.trim() ? v.trim().slice(0, max) : '');
  const num = (v, max = 1e9) => (typeof v === 'number' && Number.isFinite(v) && v >= 0 && v <= max ? v : null);
  const int = (v, min, max) => (Number.isInteger(v) && v >= min && v <= max ? v : null);
  const https = (v) => (typeof v === 'string' && v.length <= 4000 && /^https:\/\/[^\s"'<>]+$/.test(v) ? v : '');
  const list = (v, max, n) => (Array.isArray(v) ? v : []).map((x) => str(x, max)).filter(Boolean).slice(0, n);
  /** Airport codes from an array or a comma-separated string. */
  const codes = (v) => (Array.isArray(v) ? v : typeof v === 'string' ? v.split(',') : []).map((c) => str(c, 40)).filter(Boolean).slice(0, 8);

  /** A local time "2026-12-01T15:40" (or a day "2026-12-01") as its parts, without any zone. */
  function local(v) {
    const m = typeof v === 'string' && /^(\d{4})-(\d{2})-(\d{2})(?:[T ](\d{2}):(\d{2}))?/.exec(v);
    if (!m) return null;
    const [y, mo, d, h, mi] = m.slice(1).map((x) => (x === undefined ? null : Number(x)));
    if (mo < 1 || mo > 12 || d < 1 || d > 31 || (h !== null && (h > 23 || mi > 59))) return null;
    // Kept as a UTC instant and always formatted in UTC, so the wall-clock time stays as given.
    return { day: Date.UTC(y, mo - 1, d), at: h === null ? null : new Date(Date.UTC(y, mo - 1, d, h, mi)) };
  }

  function airport(a) {
    const o = a && typeof a === 'object' ? a : {};
    return { code: str(o.airport, 40), name: str(o.name), time: local(o.time) };
  }

  function segment(s) {
    if (!s || typeof s !== 'object') return null;
    return {
      flight: str(s.flight, 20), airline: str(s.airline, 80), logo: https(s.logo),
      from: airport(s.from), to: airport(s.to),
      minutes: num(s.duration_min, 1e5), aircraft: str(s.aircraft, 80), cabin: str(s.cabin, 40),
      legroom: str(s.legroom, 20), overnight: s.overnight === true, oftenDelayed: s.often_delayed === true,
    };
  }

  function layover(l) {
    if (!l || typeof l !== 'object') return null;
    return { code: str(l.airport, 40), name: str(l.name), minutes: num(l.duration_min, 1e5), overnight: l.overnight === true };
  }

  function option(o) {
    if (!o || typeof o !== 'object') return null;
    const segments = (Array.isArray(o.segments) ? o.segments : []).slice(0, MAX_SEGMENTS).map(segment).filter(Boolean);
    const depart = airport(o.depart);
    const arrive = airport(o.arrive);
    if (!depart.code && !segments.length) return null;
    return {
      topPick: o.top_pick === true,
      price: num(o.price),
      airlines: list(o.airlines, 80, 4),
      logo: https(o.logo),
      depart, arrive,
      minutes: num(o.duration_min, 1e5),
      stops: int(o.stops, 0, 20) ?? Math.max(0, segments.length - 1),
      segments,
      layovers: (Array.isArray(o.layovers) ? o.layovers : []).slice(0, MAX_SEGMENTS).map(layover).filter(Boolean),
      co2: num(o.co2_kg, 1e6),
      co2Pct: int(o.co2_vs_typical_pct, -100, 1000),
    };
  }

  function seller(s) {
    if (!s || typeof s !== 'object') return null;
    const name = str(s.seller, 80);
    if (!name) return null;
    return {
      name, part: PARTS[s.part] ? s.part : 'together', price: num(s.price), fare: str(s.fare, 80),
      bags: list(s.bags, 120, 6), marketedAs: list(s.marketed_as, 20, 6),
    };
  }

  function insights(i) {
    if (!i || typeof i !== 'object') return null;
    const lowest = num(i.lowest);
    const typical = Array.isArray(i.typical) && i.typical.length === 2 && num(i.typical[0]) !== null && num(i.typical[1]) !== null
      ? [Math.min(...i.typical), Math.max(...i.typical)] : null;
    const history = (Array.isArray(i.history) ? i.history : []).slice(-MAX_HISTORY)
      .map((p) => ({ day: local(p?.date), price: num(p?.price) }))
      .filter((p) => p.day && p.price !== null);
    const level = str(i.level, 40);
    if (lowest === null && !typical && !level) return null;
    return { lowest, level, typical, history: history.length >= 2 ? history : [] };
  }

  /** The flights in a `search_flights` result, or null if `output` isn't one. */
  function parse(output) {
    let data;
    try { data = JSON.parse(output); } catch { return null; }
    if (!data || typeof data !== 'object' || (data.kind !== 'flights' && data.kind !== 'booking')) return null;
    if (!Array.isArray(data.options)) return null;
    const from = codes(data.from);
    const to = codes(data.to);
    if (!from.length || !to.length) return null;
    const p = data.passengers && typeof data.passengers === 'object' ? data.passengers : {};
    const passengers = Object.fromEntries(PASSENGERS.map(([k]) => [k, int(p[k], 0, 99) ?? 0]));
    if (!Object.values(passengers).some(Boolean)) passengers.adults = 1;
    const currency = /^[A-Za-z]{3}$/.test(str(data.currency, 3)) ? str(data.currency, 3).toUpperCase() : 'USD';
    const booking = data.kind === 'booking';
    const options = booking ? [] : data.options.slice(0, MAX_OPTIONS).map(option).filter(Boolean);
    const sellers = booking ? data.options.slice(0, MAX_SELLERS).map(seller).filter(Boolean) : [];
    const chosen = booking ? (Array.isArray(data.flights) ? data.flights : []).slice(0, 4).map(option).filter(Boolean) : [];
    const asOf = Date.parse(str(data.as_of, 40));
    return {
      kind: data.kind,
      leg: ['one_way', 'outbound', 'return'].includes(data.leg) ? data.leg : 'one_way',
      from, to,
      date: local(data.date),
      returnDate: local(data.return_date),
      passengers,
      cabin: str(data.cabin, 40),
      currency,
      options, sellers, chosen,
      more: int(data.more, 0, 1000) ?? 0,
      noResults: Boolean(str(data.no_results, 600)) || (booking ? !sellers.length : !options.length),
      insights: booking ? null : insights(data.insights),
      url: https(data.google_flights_url),
      asOf: Number.isFinite(asOf) ? new Date(asOf) : null,
    };
  }

  // -- formatting -----------------------------------------------------------------------------

  /** A price in the result's currency, without cents when it's whole. '—' when there is none. */
  function money(x, currency) {
    if (x === null) return '—';
    const whole = Number.isInteger(x);
    const opts = { minimumFractionDigits: whole ? 0 : 2, maximumFractionDigits: whole ? 0 : 2 };
    try {
      return new Intl.NumberFormat(cedarI18n.locale, { ...opts, style: 'currency', currency }).format(x);
    } catch {
      return `${cedarI18n.number(x, opts)} ${currency}`;
    }
  }

  function moneyRange(lo, hi, currency) {
    try {
      const whole = Number.isInteger(lo) && Number.isInteger(hi);
      return new Intl.NumberFormat(cedarI18n.locale, {
        style: 'currency', currency, minimumFractionDigits: whole ? 0 : 2, maximumFractionDigits: whole ? 0 : 2,
      }).formatRange(lo, hi);
    } catch {
      return `${money(lo, currency)}–${money(hi, currency)}`;
    }
  }

  /** A wall-clock time as given (formatted in UTC, where it was stored), 12 or 24 h by locale. */
  const clock = (t) => t.at.toLocaleTimeString(cedarI18n.locale, { hour: 'numeric', minute: '2-digit', timeZone: 'UTC' });
  const day = (t, opts = { weekday: 'short', month: 'short', day: 'numeric' }) => new Date(t.day).toLocaleDateString(cedarI18n.locale, { ...opts, timeZone: 'UTC' });
  /** How many days after `a` the day of `b` is (the "+1" of an arrival after midnight). */
  const daysAfter = (a, b) => (a && b ? Math.round((b.day - a.day) / 86400000) : 0);

  function duration(min) {
    const m = Math.round(min);
    if (m < 60) return tr('flights.min', { min: m });
    const h = Math.floor(m / 60);
    return m % 60 ? tr('flights.hMin', { h, min: m % 60 }) : tr('flights.h', { h });
  }

  const route = (spec) => `${spec.from.join('/')} → ${spec.to.join('/')}`;

  function passengersText(p) {
    const parts = PASSENGERS.filter(([k]) => p[k] > 0).map(([k, key]) => tr(key, { count: p[k] }));
    try { return new Intl.ListFormat(cedarI18n.locale, { type: 'unit', style: 'short' }).format(parts); } catch { return parts.join(', '); }
  }

  const cabinText = (c) => (CABINS[c] ? tr(CABINS[c]) : c.replace(/_/g, ' '));

  function stopsText(o) {
    if (!o.stops) return tr('flights.nonstop');
    const via = o.layovers.map((l) => l.code).filter(Boolean);
    return via.length
      ? tr('flights.stopsVia', { count: o.stops, airports: via.join(', ') })
      : tr('flights.stops', { count: o.stops });
  }

  // -- pieces ---------------------------------------------------------------------------------

  /** An airline logo that drops out, leaving a plane, if it can't be loaded. */
  function logo(url, alt) {
    const box = el('span', 'cedar-flights-logo');
    const fallback = () => { box.replaceChildren(icon('plane')); box.classList.add('empty'); };
    if (!url) { fallback(); return box; }
    const img = el('img');
    img.alt = alt;
    img.decoding = 'async';
    img.loading = 'lazy';
    img.referrerPolicy = 'no-referrer';
    img.addEventListener('error', fallback);
    img.src = url;
    box.append(img);
    return box;
  }

  function flag(text, iconName, tone = '') {
    const f = el('span', `cedar-flights-flag${tone ? ` ${tone}` : ''}`);
    if (iconName) f.append(icon(iconName));
    f.append(el('span', '', text));
    return f;
  }

  /** A time, with "+1" when its day is after `from`'s. */
  function timeEl(t, from) {
    const span = el('span', 'cedar-flights-time', t?.at ? clock(t) : '–');
    const plus = daysAfter(from, t);
    if (plus > 0) {
      const sup = el('sup', 'cedar-flights-plus', `+${plus}`);
      sup.title = tr('flights.nextDay', { count: plus, date: day(t) });
      span.append(sup);
    }
    return span;
  }

  /** The small flags of an option: often delayed, overnight, its CO2 against the typical. */
  function flags(o) {
    const out = [];
    if (o.topPick) out.push(flag(tr('flights.topPick'), 'star', 'pick'));
    if (o.segments.some((s) => s.oftenDelayed)) out.push(flag(tr('flights.oftenDelayed'), 'clock', 'warn'));
    if (o.segments.some((s) => s.overnight) || o.layovers.some((l) => l.overnight)) out.push(flag(tr('flights.overnight'), 'moon'));
    if (o.co2Pct !== null && o.co2Pct <= -10) out.push(flag(tr('flights.co2.less', { pct: Math.abs(o.co2Pct) }), 'leaf', 'good'));
    else if (o.co2Pct !== null && o.co2Pct >= 10) out.push(flag(tr('flights.co2.more', { pct: o.co2Pct }), 'leaf'));
    return out;
  }

  function priceEl(spec, price, hint) {
    const box = el('span', 'cedar-flights-price');
    const amount = el('span', `cedar-flights-amount${price === null ? ' none' : ''}`, money(price, spec.currency));
    if (price === null) amount.title = tr('flights.priceUnavailable');
    box.append(amount);
    if (hint) box.append(el('span', 'cedar-flights-price-hint', hint));
    return box;
  }

  /** The segments and layovers of an option, as a timeline. */
  function detail(spec, o) {
    const wrap = el('div', 'cedar-flights-detail');
    const steps = el('ol', 'cedar-flights-steps');
    const start = o.segments[0]?.from.time || o.depart.time;
    o.segments.forEach((s, i) => {
      const seg = el('li', 'cedar-flights-seg');
      const stop = (a, cls) => {
        const row = el('div', `cedar-flights-stop ${cls}`);
        row.append(timeEl(a.time, start));
        const where = el('span', 'cedar-flights-where');
        where.append(el('strong', '', a.code || '?'));
        if (a.name) where.append(el('span', '', a.name));
        row.append(where);
        return row;
      };
      const info = el('div', 'cedar-flights-seg-info');
      const line = el('div', 'cedar-flights-seg-line');
      if (s.logo) line.append(logo(s.logo, s.airline));
      line.append(el('span', '', [s.airline, s.flight].filter(Boolean).join(' · ') || tr('flights.flight')));
      info.append(line);
      const facts = [
        s.minutes !== null ? duration(s.minutes) : '', s.aircraft, s.cabin,
        s.legroom ? tr('flights.legroom', { legroom: s.legroom }) : '',
      ].filter(Boolean);
      if (facts.length) info.append(el('div', 'cedar-flights-seg-facts', facts.join(' · ')));
      const segFlags = [];
      if (s.oftenDelayed) segFlags.push(flag(tr('flights.oftenDelayed'), 'clock', 'warn'));
      if (s.overnight) segFlags.push(flag(tr('flights.overnight'), 'moon'));
      if (segFlags.length) {
        const f = el('div', 'cedar-flights-flags');
        f.append(...segFlags);
        info.append(f);
      }
      seg.append(stop(s.from, 'dep'), info, stop(s.to, 'arr'));
      steps.append(seg);
      const l = o.layovers[i];
      if (l && i < o.segments.length - 1) {
        const lay = el('li', `cedar-flights-layover${l.overnight ? ' overnight' : ''}`);
        lay.append(icon(l.overnight ? 'moon' : 'clock'));
        const text = l.minutes !== null
          ? tr('flights.layover', { duration: duration(l.minutes), airport: l.name || l.code })
          : tr('flights.layoverAt', { airport: l.name || l.code });
        lay.append(el('span', '', text));
        if (l.overnight) lay.append(el('span', 'cedar-flights-lay-tag', tr('flights.overnight')));
        steps.append(lay);
      }
    });
    wrap.append(steps);
    const extra = [];
    if (o.co2 !== null) extra.push(tr('flights.co2.kg', { kg: o.co2 }));
    if (o.co2Pct !== null) {
      extra.push(o.co2Pct === 0 ? tr('flights.co2.typical')
        : o.co2Pct < 0 ? tr('flights.co2.less', { pct: -o.co2Pct }) : tr('flights.co2.more', { pct: o.co2Pct }));
    }
    if (extra.length) wrap.append(el('div', 'cedar-flights-co2', extra.join(' · ')));
    return wrap;
  }

  let ids = 0;

  /** One flight: a button with the summary that opens its segments and layovers. */
  function flightRow(spec, o, hint) {
    const item = el('div', 'cedar-flights-row');
    item.setAttribute('role', 'listitem');
    if (o.topPick) item.classList.add('top');
    const btn = el('button', 'cedar-flights-sum');
    btn.type = 'button';
    btn.setAttribute('aria-expanded', 'false');
    const panelId = `cedar-flights-${++ids}`;
    btn.setAttribute('aria-controls', panelId);

    const airlines = o.airlines.join(', ') || o.segments[0]?.airline || '';
    btn.append(logo(o.logo || o.segments[0]?.logo || '', airlines));

    const main = el('span', 'cedar-flights-main');
    const times = el('span', 'cedar-flights-times');
    times.append(timeEl(o.depart.time, o.depart.time), el('span', 'cedar-flights-dash', '–'), timeEl(o.arrive.time, o.depart.time));
    main.append(times);
    main.append(el('span', 'cedar-flights-sub', [airlines, `${o.depart.code}–${o.arrive.code}`].filter(Boolean).join(' · ')));
    const fl = flags(o);
    if (fl.length) {
      const f = el('span', 'cedar-flights-flags');
      f.append(...fl);
      main.append(f);
    }

    const mid = el('span', 'cedar-flights-mid');
    if (o.minutes !== null) mid.append(el('span', 'cedar-flights-dur', duration(o.minutes)));
    mid.append(el('span', `cedar-flights-stopcount${o.stops ? '' : ' nonstop'}`, stopsText(o)));

    btn.append(main, mid, priceEl(spec, o.price, hint), icon('chevron', 'cedar-flights-icon cedar-flights-chev'));

    const panel = detail(spec, o);
    panel.id = panelId;
    panel.hidden = true;
    btn.addEventListener('click', () => {
      const open = btn.getAttribute('aria-expanded') !== 'true';
      btn.setAttribute('aria-expanded', String(open));
      item.classList.toggle('open', open);
      panel.hidden = !open;
    });
    item.append(btn, panel);
    return item;
  }

  /** The price history as a small line, with the typical range as a band behind it. */
  function sparkline(spec, ins) {
    const W = 160; const H = 40; const P = 3;
    const prices = ins.history.map((p) => p.price);
    const values = ins.typical ? [...prices, ...ins.typical] : prices;
    const lo = Math.min(...values); const hi = Math.max(...values);
    const span = hi - lo || 1;
    const first = ins.history[0].day.day; const last = ins.history[ins.history.length - 1].day.day;
    const x = (d) => P + ((d - first) / (last - first || 1)) * (W - 2 * P);
    const y = (v) => H - P - ((v - lo) / span) * (H - 2 * P);
    const ns = 'http://www.w3.org/2000/svg';
    const svg = document.createElementNS(ns, 'svg');
    svg.setAttribute('viewBox', `0 0 ${W} ${H}`);
    svg.setAttribute('class', 'cedar-flights-spark');
    svg.setAttribute('role', 'img');
    const now = ins.history[ins.history.length - 1].price;
    svg.setAttribute('aria-label', tr('flights.history', {
      days: Math.round((last - first) / 86400000), from: money(ins.history[0].price, spec.currency), to: money(now, spec.currency),
    }));
    if (ins.typical) {
      const band = document.createElementNS(ns, 'rect');
      band.setAttribute('x', '0');
      band.setAttribute('width', String(W));
      band.setAttribute('y', y(ins.typical[1]).toFixed(1));
      band.setAttribute('height', Math.max(1, y(ins.typical[0]) - y(ins.typical[1])).toFixed(1));
      band.setAttribute('class', 'band');
      svg.append(band);
    }
    const line = document.createElementNS(ns, 'polyline');
    line.setAttribute('points', ins.history.map((p) => `${x(p.day.day).toFixed(1)},${y(p.price).toFixed(1)}`).join(' '));
    line.setAttribute('class', 'line');
    const dot = document.createElementNS(ns, 'circle');
    dot.setAttribute('cx', x(last).toFixed(1));
    dot.setAttribute('cy', y(now).toFixed(1));
    dot.setAttribute('r', '2.5');
    dot.setAttribute('class', 'dot');
    svg.append(line, dot);
    return svg;
  }

  function insightBand(spec) {
    const ins = spec.insights;
    const band = el('div', `cedar-flights-insight${LEVELS[ins.level] ? ` ${ins.level}` : ''}`);
    const text = el('div', 'cedar-flights-insight-text');
    if (ins.level) text.append(el('strong', '', LEVELS[ins.level] ? tr(LEVELS[ins.level]) : tr('flights.level.other', { level: ins.level })));
    const facts = [];
    if (ins.typical) facts.push(tr('flights.typical', { range: moneyRange(ins.typical[0], ins.typical[1], spec.currency) }));
    if (ins.lowest !== null) facts.push(tr('flights.lowest', { price: money(ins.lowest, spec.currency) }));
    if (facts.length) text.append(el('div', 'cedar-flights-insight-sub', facts.join(' · ')));
    band.append(text);
    if (ins.history.length >= 2) band.append(sparkline(spec, ins));
    return band;
  }

  function link(url, text, primary) {
    const a = el('a', `cedar-flights-link${primary ? ' primary' : ''}`);
    a.href = url;
    a.target = '_blank';
    a.rel = 'noopener noreferrer';
    a.append(el('span', '', text), icon('out'));
    return a;
  }

  function sellerRow(spec, s) {
    const row = el('li', 'cedar-flights-seller');
    const main = el('div', 'cedar-flights-seller-main');
    main.append(el('div', 'cedar-flights-seller-name', s.name));
    const sub = [s.fare, s.marketedAs.length ? s.marketedAs.join(', ') : ''].filter(Boolean).join(' · ');
    if (sub) main.append(el('div', 'cedar-flights-sub', sub));
    if (s.part !== 'together') main.append(flag(tr(PARTS[s.part]), '', 'warn'));
    if (s.bags.length) {
      const bags = el('ul', 'cedar-flights-bags');
      for (const b of s.bags) {
        const li = el('li');
        li.append(icon('bag'), el('span', '', b));
        bags.append(li);
      }
      main.append(bags);
    }
    row.append(main, priceEl(spec, s.price, ''));
    return row;
  }

  // -- the card -------------------------------------------------------------------------------

  /** The card for `spec`. */
  function element(spec) {
    const booking = spec.kind === 'booking';
    const card = el('div', `cedar-flights${booking ? ' booking' : ''}`);
    card.setAttribute('role', 'group');
    const fromText = spec.from.join('/'); const toText = spec.to.join('/');
    card.setAttribute('aria-label', tr(booking ? 'flights.bookingAria' : 'flights.aria', { from: fromText, to: toText }));

    // The head: the route, the airports' names when known, the day, the passengers and cabin.
    const head = el('div', 'cedar-flights-head');
    const badge = el('span', 'cedar-flights-badge');
    badge.append(icon('plane'));
    const titles = el('div', 'cedar-flights-titles');
    const title = el('div', 'cedar-flights-title');
    title.append(el('span', '', route(spec)));
    if (booking) title.append(el('span', 'cedar-flights-kind', tr('flights.bookingTitle')));
    titles.append(title);
    const sample = booking ? spec.chosen[0] : spec.options[0];
    const lastSample = booking ? spec.chosen[spec.chosen.length - 1] : sample;
    const names = [
      spec.from.length === 1 && sample?.depart.code === spec.from[0] ? sample.depart.name : '',
      spec.to.length === 1 && lastSample?.arrive.code === spec.to[0] ? lastSample.arrive.name : '',
    ];
    if (names[0] && names[1]) titles.append(el('div', 'cedar-flights-names', `${names[0]} → ${names[1]}`));
    const meta = [];
    if (spec.date) meta.push(day(spec.date, { weekday: 'short', day: 'numeric', month: 'short', year: 'numeric' }));
    if (spec.leg === 'outbound') meta.push(spec.returnDate ? tr('flights.roundTrip', { date: day(spec.returnDate) }) : tr('flights.roundTripPlain'));
    else if (spec.leg === 'return') meta.push(tr('flights.returnFlight'));
    else meta.push(tr('flights.oneWay'));
    meta.push(passengersText(spec.passengers));
    if (spec.cabin) meta.push(cabinText(spec.cabin));
    titles.append(el('div', 'cedar-flights-meta', meta.join(' · ')));
    head.append(badge, titles);
    card.append(head);

    if (spec.insights) card.append(insightBand(spec));

    // Google's prices on a round trip cover both ways.
    const hint = spec.leg === 'one_way' ? '' : tr('flights.roundTripPrice');
    if (booking) {
      if (spec.chosen.length) {
        const chosen = el('div', 'cedar-flights-section');
        chosen.append(el('div', 'cedar-flights-label', tr('flights.chosen', { count: spec.chosen.length })));
        const rows = el('div', 'cedar-flights-list');
        rows.setAttribute('role', 'list');
        for (const o of spec.chosen) rows.append(flightRow(spec, o, ''));
        chosen.append(rows);
        card.append(chosen);
      }
      const where = el('div', 'cedar-flights-section');
      where.append(el('div', 'cedar-flights-label', tr('flights.sellers')));
      if (spec.sellers.length) {
        const ul = el('ul', 'cedar-flights-sellers');
        for (const s of spec.sellers) ul.append(sellerRow(spec, s));
        where.append(ul);
      } else {
        where.append(el('div', 'cedar-flights-empty', tr('flights.noSellers')));
      }
      card.append(where);
    } else if (spec.options.length) {
      const rows = el('div', 'cedar-flights-list');
      rows.setAttribute('role', 'list');
      for (const o of spec.options) rows.append(flightRow(spec, o, hint));
      card.append(rows);
      if (spec.more > 0) card.append(el('div', 'cedar-flights-more', tr('flights.more', { count: spec.more })));
    } else {
      card.append(el('div', 'cedar-flights-empty', tr('flights.noResults')));
    }

    // The foot: the link to Google Flights (where booking starts) and how fresh the prices are.
    const foot = el('div', 'cedar-flights-foot');
    if (spec.url) foot.append(link(spec.url, tr(booking ? 'flights.book' : 'flights.open'), booking));
    const fresh = spec.asOf
      ? tr('flights.asOf', { time: spec.asOf.toLocaleString(cedarI18n.locale, { month: 'short', day: 'numeric', hour: 'numeric', minute: '2-digit' }) })
      : tr('flights.fresh');
    foot.append(el('span', 'cedar-flights-asof', fresh));
    card.append(foot);
    return card;
  }

  return { parse, element };
})();

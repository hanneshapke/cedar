'use strict';

// Weather display for cedar's `weather_forecast` tool, shared by every UI. The tool returns the
// plain-text report the model reads (see src/tools/weather.rs, `format_forecast`):
//
//   Berlin, State of Berlin, Germany (52.52, 13.41), timezone Europe/Berlin
//
//   Now (2026-10-12 21:00): clear sky, 10.2°C (feels 8.3°C), humidity 70%, wind 4.6 km/h from NW, ...
//
//   Mon 2026-10-12: overcast, low 6.8°C, high 13.7°C, precipitation 0 mm (32% chance), ...
//   Tue 2026-10-13: ...
//
//   Weather data by Open-Meteo.com (CC BY 4.0)
//
// and this reads it back into a card: the place, the current conditions with a large icon, the
// key numbers (feels like, humidity, wind with its direction, gusts, precipitation, UV, sunrise and
// sunset), and one row per day with its icon, chance of rain and a temperature bar on a shared
// scale. A day row opens to show that day's details. Any part the report leaves out is skipped.
//
//   cedarWeather.parse(output) -> spec | null   the forecast, or null if `output` isn't one
//   cedarWeather.element(spec) -> HTMLElement   the card
//
// It needs nothing from the app. The text came from the network, so it is only ever set as text.

const cedarWeather = (() => {
  const MAX_DAYS = 16;
  const MAX_TEXT = 200;
  const OPEN_METEO = 'https://open-meteo.com/';

  // The words weather.rs writes for each WMO weather code, read back to the code.
  const CODES = {
    'clear sky': 0, 'mainly clear': 1, 'partly cloudy': 2, overcast: 3, fog: 45, 'freezing fog': 48,
    'light drizzle': 51, 'moderate drizzle': 53, 'dense drizzle': 55, 'light freezing drizzle': 56,
    'dense freezing drizzle': 57, 'slight rain': 61, 'moderate rain': 63, 'heavy rain': 65,
    'light freezing rain': 66, 'heavy freezing rain': 67, 'slight snow': 71, 'moderate snow': 73,
    'heavy snow': 75, 'snow grains': 77, 'slight rain showers': 80, 'moderate rain showers': 81,
    'violent rain showers': 82, 'slight snow showers': 85, 'heavy snow showers': 86, thunderstorm: 95,
    'thunderstorm with slight hail': 96, 'thunderstorm with heavy hail': 99,
  };
  const COMPASS = ['N', 'NNE', 'NE', 'ENE', 'E', 'ESE', 'SE', 'SSE', 'S', 'SSW', 'SW', 'WSW', 'W', 'WNW', 'NW', 'NNW'];

  const NUM = '-?\\d+(?:\\.\\d+)?(?:e-?\\d+)?';
  const QTY_RE = new RegExp(`^(${NUM})\\s*(.*)$`);
  const HEADER_RE = new RegExp(`^(?:(.+?) \\()?(${NUM}), (${NUM})\\)?(?:, timezone (\\S+))?$`);
  const NOW_RE = /^Now(?: \((\d{4}-\d{2}-\d{2}) (\d{2}:\d{2})\))?: (.+)$/;
  const DAY_RE = /^[A-Z][a-z]{2} (\d{4}-\d{2}-\d{2}): (.+)$/;
  const CLOCK_RE = /^\d{2}:\d{2}$/;

  function el(tag, className, text) {
    const e = document.createElement(tag);
    if (className) e.className = className;
    if (text !== undefined) e.textContent = text;
    return e;
  }

  const clip = (s) => String(s).trim().slice(0, MAX_TEXT);

  /** `{value, unit}` from "10.2°C", "4.6 km/h", "70%", "2.9"; null if it doesn't start with a number. */
  function quantity(s) {
    const m = String(s).trim().match(QTY_RE);
    if (!m) return null;
    const value = Number(m[1]);
    return Number.isFinite(value) ? { value, unit: clip(m[2]) } : null;
  }

  /** The weather code for a condition in words ("partly cloudy", "weather code 4"), or undefined. */
  function codeOf(words) {
    if (words in CODES) return CODES[words];
    const m = words.match(/^weather code (-?\d+)$/);
    return m ? Number(m[1]) : undefined;
  }

  /** The current conditions from what follows "Now (...): ". */
  function parseNow(text) {
    const now = {};
    for (const part of text.split(', ')) {
      let m;
      const code = codeOf(part);
      if (code !== undefined && now.code === undefined) now.code = code;
      else if ((m = part.match(/^(.+?) \(feels (.+)\)$/))) { now.temp = quantity(m[1]); now.feels = quantity(m[2]); }
      else if ((m = part.match(/^humidity (.+)$/))) now.humidity = quantity(m[1]);
      else if ((m = part.match(/^wind (.+?)(?: from ([NESW]{1,3}))?$/))) { now.wind = quantity(m[1]); if (COMPASS.includes(m[2])) now.from = m[2]; }
      else if ((m = part.match(/^gusts (.+)$/))) now.gusts = quantity(m[1]);
      else if ((m = part.match(/^precipitation (.+)$/))) now.precip = quantity(m[1]);
      else if (/°/.test(part) && !now.temp) now.temp = quantity(part);
    }
    return now;
  }

  /** One day from what follows "Mon 2026-10-12: ". */
  function parseDay(date, text) {
    const day = { date };
    for (const part of text.split(', ')) {
      let m;
      const code = codeOf(part);
      if (code !== undefined && day.code === undefined) day.code = code;
      else if ((m = part.match(/^low (.+)$/))) day.low = quantity(m[1]);
      else if ((m = part.match(/^high (.+)$/))) day.high = quantity(m[1]);
      else if ((m = part.match(/^precipitation (.+?)(?: \((.+) chance\))?$/))) { day.precip = quantity(m[1]); if (m[2]) day.chance = quantity(m[2]); }
      else if ((m = part.match(/^wind up to (.+?)(?: \(gusts (.+)\))?$/))) { day.wind = quantity(m[1]); if (m[2]) day.gusts = quantity(m[2]); }
      else if ((m = part.match(/^UV (.+)$/))) day.uv = quantity(m[1]);
      else if ((m = part.match(/^sunrise (.+)$/)) && CLOCK_RE.test(m[1])) day.sunrise = m[1];
      else if ((m = part.match(/^sunset (.+)$/)) && CLOCK_RE.test(m[1])) day.sunset = m[1];
    }
    return day;
  }

  /** The forecast in a `weather_forecast` result, or null if `output` isn't one. */
  function parse(output) {
    if (typeof output !== 'string') return null;
    const lines = output.split('\n').map((l) => l.trim()).filter(Boolean);
    const head = lines[0]?.match(HEADER_RE);
    if (!head) return null;
    const spec = {
      label: head[1] ? clip(head[1]) : '',
      latitude: Number(head[2]),
      longitude: Number(head[3]),
      timezone: head[4] ? clip(head[4]) : '',
      now: null,
      days: [],
    };
    for (const line of lines.slice(1)) {
      let m;
      if (!spec.now && (m = line.match(NOW_RE))) {
        spec.now = parseNow(m[3]);
        if (m[1]) { spec.now.date = m[1]; spec.now.time = m[2]; }
      } else if ((m = line.match(DAY_RE)) && spec.days.length < MAX_DAYS) {
        spec.days.push(parseDay(m[1], m[2]));
      }
    }
    if (!spec.now?.temp && !spec.days.some((d) => d.low || d.high)) return null;
    return spec;
  }

  // ---- drawing ----------------------------------------------------------------------------------

  // Line icons on a 24-unit grid, drawn with currentColor plus a few tinted parts (classes in the CSS).
  const SUN = '<circle class="cw-i-sun" cx="12" cy="12" r="4"/><path class="cw-i-ray" d="M12 2.5v2M12 19.5v2M4.6 4.6l1.4 1.4M18 18l1.4 1.4M2.5 12h2M19.5 12h2M4.6 19.4L6 18M18 6l1.4-1.4"/>';
  const MOON = '<path class="cw-i-moon" d="M19.5 14.5A8 8 0 0 1 9.5 4.5a8 8 0 1 0 10 10z"/>';
  const SMALL_SUN = '<circle class="cw-i-sun" cx="8.5" cy="8.5" r="3"/><path class="cw-i-ray" d="M8.5 2.5v1.3M2.5 8.5h1.3M4.3 4.3l.9.9M12.7 4.3l-.9.9M4.3 12.7l.9-.9"/>';
  const SMALL_MOON = '<path class="cw-i-moon" d="M12 9.5A4.8 4.8 0 0 1 6.5 4a4.8 4.8 0 1 0 5.5 5.5z"/>';
  const CLOUD = '<path class="cw-i-cloud" d="M7 18.5a4 4 0 0 1-.4-8 5.5 5.5 0 0 1 10.6 1.3 3.4 3.4 0 0 1 .3 6.7z"/>';
  const LOW_CLOUD = '<path class="cw-i-cloud" d="M7 15a4 4 0 0 1-.4-8 5.5 5.5 0 0 1 10.6 1.3 3.4 3.4 0 0 1 .3 6.7z"/>';
  const ICONS = {
    clear: SUN,
    clearNight: MOON,
    partly: `${SMALL_SUN}<path class="cw-i-cloud" d="M9 19.5a3.6 3.6 0 0 1-.3-7.2 5 5 0 0 1 9.6 1.2 3 3 0 0 1 .3 6z"/>`,
    partlyNight: `${SMALL_MOON}<path class="cw-i-cloud" d="M9 19.5a3.6 3.6 0 0 1-.3-7.2 5 5 0 0 1 9.6 1.2 3 3 0 0 1 .3 6z"/>`,
    cloud: CLOUD,
    fog: '<path class="cw-i-cloud" d="M7 13a4 4 0 0 1-.4-8 5.5 5.5 0 0 1 10.6 1.3 3.4 3.4 0 0 1 .3 6.7z"/><path class="cw-i-fog" d="M4 16.5h16M6 19.5h12"/>',
    drizzle: `${LOW_CLOUD}<path class="cw-i-rain" d="M9 18.5v.5M12.5 18.5v.5M16 18.5v.5M10.5 21v.5M14 21v.5"/>`,
    rain: `${LOW_CLOUD}<path class="cw-i-rain" d="M9 17.5l-1 3M13 17.5l-1 3M17 17.5l-1 3"/>`,
    sleet: `${LOW_CLOUD}<path class="cw-i-rain" d="M9 17.5l-1 3M16 17.5l-1 3"/><path class="cw-i-snow" d="M12.5 18v3M11.2 18.8l2.6 1.4M11.2 20.2l2.6-1.4"/>`,
    snow: `${LOW_CLOUD}<path class="cw-i-snow" d="M8.5 18v3M7.2 18.8l2.6 1.4M7.2 20.2l2.6-1.4M15.5 18v3M14.2 18.8l2.6 1.4M14.2 20.2l2.6-1.4"/>`,
    storm: `${LOW_CLOUD}<path class="cw-i-bolt" d="M12.5 15l-2 3.5h3l-2 3.5"/>`,
  };
  const ARROW = '<path d="M12 4v16M6.5 9.5L12 4l5.5 5.5"/>';

  /** Which icon and sky a WMO code gets; clear and partly cloudy have a night version. */
  function sky(code, night) {
    if (code === 0 || code === 1) return { icon: night ? 'clearNight' : 'clear', sky: 'clear' };
    if (code === 2) return { icon: night ? 'partlyNight' : 'partly', sky: 'partly' };
    if (code === 3) return { icon: 'cloud', sky: 'cloud' };
    if (code === 45 || code === 48) return { icon: 'fog', sky: 'fog' };
    if (code >= 51 && code <= 57) return { icon: 'drizzle', sky: 'rain' };
    if (code === 66 || code === 67) return { icon: 'sleet', sky: 'rain' };
    if ((code >= 61 && code <= 65) || (code >= 80 && code <= 82)) return { icon: 'rain', sky: 'rain' };
    if ((code >= 71 && code <= 77) || code === 85 || code === 86) return { icon: 'snow', sky: 'snow' };
    if (code >= 95 && code <= 99) return { icon: 'storm', sky: 'storm' };
    return { icon: 'cloud', sky: 'cloud' };
  }

  function icon(name, className) {
    const span = el('span', `cw-icon ${className || ''}`.trim());
    span.innerHTML = `<svg viewBox="0 0 24 24" aria-hidden="true">${ICONS[name] || ICONS.cloud}</svg>`;
    return span;
  }

  /** The condition in the reader's language ("Partly cloudy"); unknown codes say so. */
  function condition(code) {
    if (code === undefined) return '';
    const key = `weather.code.${code}`;
    return cedarI18n.has(key) ? tr(key) : tr('weather.code.unknown', { code });
  }

  const fmt = (n, digits = 1) => cedarI18n.number(n, { maximumFractionDigits: digits });
  /** "10.2°C" → "10°"; the unit is shown once, by the current temperature. */
  const degrees = (q) => (q ? `${fmt(Math.round(q.value), 0)}°` : '–');
  /** "4.6 km/h", "0.2 mm", "70%": the number in the reader's format with its unit. */
  function amount(q, digits = 1) {
    if (!q) return '';
    const unit = q.unit === 'inch' ? 'in' : q.unit;
    if (!unit) return fmt(q.value, digits);
    return unit === '%' || unit.startsWith('°') ? `${fmt(q.value, digits)}${unit}` : `${fmt(q.value, digits)} ${unit}`;
  }

  /** A temperature in °C, whichever unit it came in, for picking a colour. */
  const celsius = (q) => (q.unit === '°F' ? (q.value - 32) * 5 / 9 : q.value);
  /** Cold blue through mild green and yellow to hot red, for a temperature in °C. */
  function tempColor(c) {
    const stops = [[-15, 230], [0, 200], [10, 160], [18, 95], [25, 40], [35, 5]];
    if (c <= stops[0][0]) return `hsl(${stops[0][1]} 75% 55%)`;
    for (let i = 1; i < stops.length; i++) {
      const [t1, h1] = stops[i];
      const [t0, h0] = stops[i - 1];
      if (c <= t1) return `hsl(${Math.round(h0 + (h1 - h0) * ((c - t0) / (t1 - t0)))} 75% 55%)`;
    }
    return `hsl(${stops[stops.length - 1][1]} 75% 55%)`;
  }

  /** "Today", "Tomorrow", else the weekday; and "12 Oct" in the reader's format. */
  function dayNames(date, index, today) {
    const d = new Date(`${date}T12:00:00Z`);
    const loc = cedarI18n.locale;
    const short = d.toLocaleDateString(loc, { day: 'numeric', month: 'short', timeZone: 'UTC' });
    const weekday = d.toLocaleDateString(loc, { weekday: 'short', timeZone: 'UTC' });
    const long = d.toLocaleDateString(loc, { weekday: 'long', day: 'numeric', month: 'long', timeZone: 'UTC' });
    let name = weekday;
    if (today ? date === today : index === 0) name = tr('weather.today');
    else if (today && Date.parse(`${date}T00:00:00Z`) - Date.parse(`${today}T00:00:00Z`) === 86400000) name = tr('weather.tomorrow');
    return { name, short, long };
  }

  /** A label-value pair for the stats grids. */
  function stat(label, value, extra) {
    const item = el('div', 'cw-stat');
    item.append(el('dt', '', label));
    const dd = el('dd', '', value);
    if (extra) dd.prepend(extra);
    item.append(dd);
    return item;
  }

  /** A small arrow pointing where the wind blows to (it comes from `from`). */
  function windArrow(from) {
    const span = el('span', 'cw-wind-arrow');
    span.innerHTML = `<svg viewBox="0 0 24 24" aria-hidden="true">${ARROW}</svg>`;
    span.style.transform = `rotate(${COMPASS.indexOf(from) * 22.5 + 180}deg)`;
    return span;
  }

  function element(spec) {
    const today = spec.now?.date || spec.days[0]?.date;
    const todayDay = spec.days.find((d) => d.date === today);
    // Night if the report's local time is outside today's sunrise to sunset.
    const night = !!(spec.now?.time && todayDay?.sunrise && todayDay?.sunset
      && (spec.now.time < todayDay.sunrise || spec.now.time >= todayDay.sunset));
    const leadCode = spec.now?.code ?? spec.days[0]?.code;
    const lead = sky(leadCode, night);

    const [name, ...rest] = (spec.label || '').split(', ');
    const placeName = name || `${fmt(spec.latitude, 2)}, ${fmt(spec.longitude, 2)}`;

    const root = el('section', 'cedar-weather');
    root.dataset.sky = lead.sky;
    if (night) root.dataset.night = '';
    root.setAttribute('aria-label', tr('weather.aria', { place: placeName }));

    // ---- now ----
    const top = el('div', 'cw-top');
    const place = el('div', 'cw-place');
    place.append(el('div', 'cw-name', placeName));
    const sub = [rest.join(', ')];
    if (spec.now?.time) sub.push(tr('weather.localTime', { time: spec.now.time }));
    place.append(el('div', 'cw-sub', sub.filter(Boolean).join(' · ')));
    top.append(place);

    const hero = el('div', 'cw-hero');
    hero.append(icon(lead.icon, 'cw-hero-icon'));
    const reading = el('div', 'cw-reading');
    const nowTemp = spec.now?.temp;
    if (nowTemp) {
      const temp = el('div', 'cw-temp', `${fmt(Math.round(nowTemp.value), 0)}`);
      temp.append(el('span', 'cw-temp-unit', nowTemp.unit || '°'));
      reading.append(temp);
    }
    const cond = el('div', 'cw-cond-block');
    cond.append(el('div', 'cw-cond', condition(leadCode)));
    const range = [];
    if (spec.now?.feels) range.push(tr('weather.feels', { temp: degrees(spec.now.feels) }));
    if (todayDay?.high && todayDay?.low) range.push(tr('weather.highLow', { high: degrees(todayDay.high), low: degrees(todayDay.low) }));
    if (range.length) cond.append(el('div', 'cw-range', range.join(' · ')));
    reading.append(cond);
    hero.append(reading);
    top.append(hero);

    const stats = el('dl', 'cw-stats');
    const now = spec.now || {};
    if (now.humidity) stats.append(stat(tr('weather.humidity'), amount(now.humidity, 0)));
    if (now.wind) {
      const label = now.from ? tr('weather.windFrom', { dir: now.from }) : tr('weather.wind');
      stats.append(stat(label, amount(now.wind), now.from ? windArrow(now.from) : null));
    }
    if (now.gusts) stats.append(stat(tr('weather.gusts'), amount(now.gusts)));
    if (now.precip) stats.append(stat(tr('weather.precipitation'), amount(now.precip, 2)));
    if (todayDay?.uv) stats.append(stat(tr('weather.uv'), fmt(todayDay.uv.value)));
    if (todayDay?.sunrise && todayDay?.sunset) stats.append(stat(tr('weather.daylight'), `${todayDay.sunrise} – ${todayDay.sunset}`));
    else if (todayDay?.sunrise) stats.append(stat(tr('weather.sunrise'), todayDay.sunrise));
    else if (todayDay?.sunset) stats.append(stat(tr('weather.sunset'), todayDay.sunset));
    if (stats.childElementCount) top.append(stats);
    root.append(top);

    // ---- days ----
    const days = spec.days.filter((d) => d.low && d.high);
    if (days.length) {
      const lows = days.map((d) => d.low.value);
      const highs = days.map((d) => d.high.value);
      const min = Math.min(...lows, nowTemp ? nowTemp.value : Infinity);
      const max = Math.max(...highs, nowTemp ? nowTemp.value : -Infinity);
      const span = Math.max(max - min, 1);
      const pct = (v) => `${(((v - min) / span) * 100).toFixed(2)}%`;
      const anyChance = days.some((d) => d.chance);

      const list = el('div', 'cw-days');
      list.append(el('div', 'cw-days-title', tr('weather.forecast', { count: days.length })));
      days.forEach((d, i) => {
        const names = dayNames(d.date, i, today);
        const item = el('div', 'cw-day');
        const row = el('button', 'cw-day-row');
        row.type = 'button';
        row.setAttribute('aria-expanded', 'false');
        const dayCode = sky(d.code, false);
        const when = el('span', 'cw-day-name', names.name);
        when.title = names.long;
        const date = el('span', 'cw-day-date', names.short);
        const ic = icon(dayCode.icon, 'cw-day-icon');
        ic.title = condition(d.code);
        const chance = el('span', 'cw-day-chance', d.chance && d.chance.value >= 10 ? amount(d.chance, 0) : '');
        const low = el('span', 'cw-day-low', degrees(d.low));
        const high = el('span', 'cw-day-high', degrees(d.high));
        const track = el('span', 'cw-bar');
        const fill = el('span', 'cw-bar-fill');
        fill.style.left = pct(d.low.value);
        fill.style.right = `calc(100% - ${pct(d.high.value)})`;
        fill.style.background = `linear-gradient(90deg, ${tempColor(celsius(d.low))}, ${tempColor(celsius(d.high))})`;
        track.append(fill);
        if (nowTemp && d.date === today) {
          const dot = el('span', 'cw-bar-now');
          dot.style.left = pct(Math.min(Math.max(nowTemp.value, min), max));
          dot.title = tr('weather.nowTemp', { temp: degrees(nowTemp) });
          track.append(dot);
        }
        row.append(when, date, ic);
        if (anyChance) row.append(chance);
        row.append(low, track, high);
        row.setAttribute('aria-label', tr('weather.dayAria', { day: names.long, cond: condition(d.code), low: degrees(d.low), high: degrees(d.high) }));
        if (!anyChance) item.classList.add('no-chance');

        const detail = el('dl', 'cw-day-detail');
        detail.hidden = true;
        detail.append(stat(tr('weather.condition'), condition(d.code) || '–'));
        if (d.precip) detail.append(stat(tr('weather.precipitation'), d.chance ? tr('weather.precipChance', { amount: amount(d.precip, 2), chance: amount(d.chance, 0) }) : amount(d.precip, 2)));
        if (d.wind) detail.append(stat(tr('weather.windMax'), d.gusts ? tr('weather.withGusts', { wind: amount(d.wind), gusts: amount(d.gusts) }) : amount(d.wind)));
        if (d.uv) detail.append(stat(tr('weather.uv'), fmt(d.uv.value)));
        if (d.sunrise && d.sunset) detail.append(stat(tr('weather.daylight'), `${d.sunrise} – ${d.sunset}`));
        else if (d.sunrise) detail.append(stat(tr('weather.sunrise'), d.sunrise));
        else if (d.sunset) detail.append(stat(tr('weather.sunset'), d.sunset));

        row.addEventListener('click', () => {
          const open = row.getAttribute('aria-expanded') !== 'true';
          row.setAttribute('aria-expanded', String(open));
          item.classList.toggle('open', open);
          detail.hidden = !open;
        });
        item.append(row, detail);
        list.append(item);
      });
      root.append(list);
    }

    // ---- credit, as Open-Meteo's CC BY 4.0 license asks ----
    const foot = el('div', 'cw-foot');
    const link = el('a', '', 'Open-Meteo.com');
    link.href = OPEN_METEO;
    link.target = '_blank';
    link.rel = 'noopener noreferrer';
    const credit = tr('weather.credit');
    const [before, after = ''] = credit.split('{source}');
    const line = el('span', 'cw-credit');
    line.append(before, link, after);
    foot.append(line);
    if (spec.timezone) foot.append(el('span', 'cw-tz', spec.timezone));
    root.append(foot);

    return root;
  }

  return { parse, element };
})();

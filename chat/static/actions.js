'use strict';

// Actions, shared by every UI: what the person showed Cedar how to do. When Cedar can't do
// something, they record their screen (or one window) while they do it themselves; the
// recording is made small here in the page, a model that can watch it writes the steps, and the
// person reads, corrects and saves them. Later runs read the steps back (cedar's `saved_actions`
// tool), so what Cedar couldn't do once it can do from then on. This is one page, drawn over the
// chat and addressed by the hash, like the Agents page:
//
//   #/actions              every saved action
//   #/actions/new          the recorder (?goal=<text> says what is about to be shown, as the
//                          card of something Cedar couldn't do hands it over)
//   #/actions/<id>         one action: its steps, its site and the passwords stored with it
//
// A recording never leaves the page as it was made:
//
//   - It is captured small: at most 1600 pixels a side, a few frames a second, a low bit rate.
//   - When it stops, the page plays it through once, fast, and keeps the frames in which the
//     screen changed (and the last one before each change, which shows what was typed).
//   - What goes to the server is the small video, for a model that takes video, or else those
//     frames, for a model that takes images. It is sent once, for the steps, and not kept.
//
// A password typed in a recording shows as dots, and the steps never hold one. The person can
// store it with the action: it is encrypted on the server, the model never reads it, and the
// browser tool types it only on the action's own site.
//
// Each app hands over its own helpers through cedarActions.init(), calls cedarActions.start()
// once the access token is known, and asks cedarActions.route() in its router (beside
// cedarAgents.route()), which says whether the hash was one of the above:
//
//   api(method, path, body) -> Promise<json>   the app's /api client (sends the access token)
//   toast(message)                             a short notice

const cedarActions = (() => {
  const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
  /** How the screen is captured: small enough to send, sharp enough to read. */
  const CAPTURE = { maxSide: 1600, frameRate: 5, bitsPerSecond: 450_000 };
  /**
   * The frames kept of a recording: their size, and how different two must be. A frame is
   * compared as a grid of `sigW` by `sigH` cells; a cell counts as different when a colour moved by
   * more than `cell` (of 255), which the noise of a compressed video doesn't reach. `changed`
   * different cells make a new picture (a page, a dialog, a list that opened), kept once it has
   * stood for `settleMs`, so a page half drawn is not; more than `settled` cells since the last
   * frame kept mean something small happened: text typed into a field, the pointer on a button.
   * A change that never comes to rest (a video on the page) is kept after `restlessMs`.
   */
  const FRAME = { maxSide: 1280, quality: 0.72, sigW: 96, sigH: 54, cell: 14, changed: 12, settled: 2, settleMs: 400, gapMs: 100, restlessMs: 1500 };
  const PLAYBACK_RATES = [8, 4, 2];
  const MIME_TYPES = ['video/webm;codecs=vp9', 'video/webm;codecs=vp8', 'video/webm', 'video/mp4'];
  const ICONS = {
    back: '<path d="M15 6l-6 6 6 6"></path>',
    action: '<circle cx="12" cy="12" r="9"></circle><circle cx="12" cy="12" r="3.2"></circle>',
    screen: '<rect x="3" y="4" width="18" height="12" rx="2"></rect><path d="M8 20h8M12 16v4"></path>',
    window: '<rect x="3" y="5" width="18" height="14" rx="2"></rect><path d="M3 9h18"></path>',
    lock: '<rect x="5" y="11" width="14" height="9" rx="2"></rect><path d="M8 11V8a4 4 0 0 1 8 0v3"></path>',
  };

  let deps = null;
  let page = null;       // the page's elements, built on first use
  let started = false;
  let info = { supported: true, actions: [], limits: {}, recording: {}, secrets: false, enabled: true, browser: true };
  let view = 0;          // bumped on every render, so a slow step can't draw into a newer view
  let lastChat = '';     // the hash to go back to
  let session = null;    // the recording being made: { stream, recorder, timer, ... }

  // -- helpers -----------------------------------------------------------------------------------

  function el(tag, className, text) {
    const e = document.createElement(tag);
    if (className) e.className = className;
    if (text !== undefined) e.textContent = text;
    return e;
  }

  function button(label, onClick, className = 'copy-btn') {
    const b = el('button', className, label);
    b.type = 'button';
    b.addEventListener('click', onClick);
    return b;
  }

  function link(label, href, className) {
    const a = el('a', className, label);
    a.href = href;
    return a;
  }

  function icon(name) {
    const span = el('span', 'cedar-ag-icon');
    span.innerHTML = `<svg viewBox="0 0 24 24" aria-hidden="true">${ICONS[name] || ICONS.action}</svg>`;
    return span;
  }

  function field(label, control, help) {
    const f = el('label', 'cedar-ag-field');
    f.append(el('span', '', label), control);
    if (help) f.append(el('small', 'cedar-act-hint', help));
    return f;
  }

  function input(value, { type = 'text', max, placeholder } = {}) {
    const i = el('input');
    i.type = type;
    if (max) i.maxLength = max;
    if (placeholder) i.placeholder = placeholder;
    i.value = value || '';
    return i;
  }

  const clock = (ms) => `${Math.floor(ms / 60000)}:${String(Math.floor(ms / 1000) % 60).padStart(2, '0')}`;
  /** A size in megabytes with one decimal; anything there is counts as at least 0.1. */
  const megabytes = (bytes) => Math.max(bytes ? 0.1 : 0, bytes / (1 << 20)).toLocaleString(cedarI18n.locale, { maximumFractionDigits: 1, minimumFractionDigits: 1 });
  /** The decoded size of a base64 string. */
  const base64Bytes = (data) => Math.floor(data.length * 3 / 4);
  const canRecord = () => !!(navigator.mediaDevices?.getDisplayMedia && window.MediaRecorder);

  // -- the page ----------------------------------------------------------------------------------

  function build() {
    // The Agents page's shell and classes (agents.css), which every UI loads.
    const root = el('section', 'cedar-ag-page cedar-act-page');
    root.id = 'actions-page';
    root.hidden = true;
    root.setAttribute('aria-labelledby', 'actions-page-title');
    const wrap = el('div', 'cedar-ag-wrap');
    const top = el('div', 'cedar-ag-top');
    const back = button('', () => leave(), 'cedar-ag-back');
    back.innerHTML = `<svg viewBox="0 0 24 24" aria-hidden="true">${ICONS.back}</svg>`;
    back.append(el('span', '', tr('actions.back')));
    top.append(back);
    const body = el('div', 'cedar-ag-body');
    wrap.append(top, body);
    root.append(wrap);
    document.body.append(root);
    return { root, body, top };
  }

  const ownHash = (hash) => /^#\/(actions|agents)(\/|\?|$)/.test(hash || '');

  /** Back to the chat that was open (or a new one). */
  function leave() {
    location.hash = lastChat && !ownHash(lastChat) ? lastChat : '#/';
  }

  function show() {
    page ??= build();
    if (page.root.hidden) {
      page.root.hidden = false;
      page.root.scrollTop = 0;
    }
  }

  function hide() {
    view++;
    stopCapture();
    if (page) page.root.hidden = true;
  }

  function heading(title, ...beside) {
    const head = el('div', 'cedar-ag-head');
    const h1 = el('h1', '', title);
    h1.id = 'actions-page-title';
    head.append(h1, ...beside);
    return head;
  }

  function crumb() {
    page.top.append(link(tr('actions.all'), '#/actions', 'cedar-ag-back'));
  }

  /**
   * Handle the hash if it is one of the page's, and say whether it was. Each app's router asks
   * this, and shows its own view only when neither this page nor the Agents page took the hash.
   */
  function route() {
    const m = location.hash.match(/^#\/actions(?:\/([^?]*))?(?:\?(.*))?$/);
    if (!m || !deps) {
      hide();
      if (!ownHash(location.hash)) lastChat = location.hash;
      return false;
    }
    const part = (m[1] || '').split('/').filter(Boolean)[0] || '';
    const query = new URLSearchParams(m[2] || '');
    view++;
    stopCapture();
    show();
    page.root.scrollTop = 0;
    page.top.replaceChildren(page.top.firstElementChild);
    if (part === 'new') viewNew(query.get('goal') || '');
    else if (UUID.test(part)) viewAction(part);
    else viewList();
    return true;
  }

  // -- every action ------------------------------------------------------------------------------

  /** Fetch the actions and what this server can do with a recording; keep the sidebar link current. */
  async function load() {
    if (!deps) return false;
    try {
      const data = await deps.api('GET', '/actions');
      info = { supported: data.supported !== false, actions: data.actions || [], limits: data.limits || {}, recording: data.recording || {}, secrets: !!data.secrets, enabled: data.enabled !== false, browser: data.browser !== false };
    } catch { return false; }
    renderNav();
    return true;
  }

  function renderNav() {
    for (const nav of document.querySelectorAll('[data-actions-nav]')) {
      nav.hidden = !info.supported;
      const count = nav.querySelector('.cedar-ag-count');
      if (count) count.textContent = info.actions.length ? String(info.actions.length) : '';
    }
  }

  /** Why nothing here can watch a recording, or '' when something can. */
  function cannotWatch() {
    return info.recording.video || info.recording.frames ? '' : tr('actions.noModel');
  }

  function card(a) {
    const c = link('', `#/actions/${a.id}`, 'cedar-ag-card');
    const title = el('div', 'cedar-ag-card-title');
    title.append(el('span', '', a.title));
    const meta = el('div', 'cedar-ag-meta');
    if (a.site) meta.append(el('span', '', a.site));
    meta.append(el('span', '', tr('actions.steps', { count: a.steps.length })));
    if (a.secrets.some((s) => s.saved)) meta.append(el('span', 'cedar-act-locked', tr('actions.passwordStored')));
    meta.append(el('span', '', a.uses ? tr('actions.used', { count: a.uses }) : tr('actions.unused')));
    c.append(icon('action'), title);
    if (a.summary) c.append(el('div', 'cedar-ag-sub', a.summary));
    c.append(meta);
    return c;
  }

  async function viewList() {
    const current = view;
    const draw = () => {
      const body = page.body;
      body.replaceChildren(heading(tr('actions.title'), link(tr('actions.record'), '#/actions/new', 'primary')), el('p', 'cedar-ag-intro', tr('actions.intro')));
      if (!info.supported) { body.append(el('div', 'cedar-ag-empty', tr('actions.unsupported'))); return; }
      if (!info.enabled) body.append(el('div', 'cedar-ag-box warn', tr('actions.off')));
      if (!info.actions.length) {
        const empty = el('div', 'cedar-ag-empty');
        empty.append(el('div', '', tr('actions.empty')), link(tr('actions.record'), '#/actions/new', 'primary'));
        body.append(empty);
        return;
      }
      const cards = el('div', 'cedar-ag-cards');
      for (const a of [...info.actions].reverse()) cards.append(card(a));
      body.append(cards);
    };
    if (started) draw(); else page.body.replaceChildren(heading(tr('actions.title')), el('p', 'cedar-ag-note', tr('actions.loading')));
    const ok = await load();
    if (current !== view) return;
    if (ok) draw(); else page.body.append(el('div', 'cedar-ag-error', tr('actions.loadFailed')));
  }

  // -- capturing the screen -------------------------------------------------------------------------

  /** Stop a capture under way and let go of its screen share. */
  function stopCapture() {
    if (!session) return;
    const s = session;
    session = null;
    clearInterval(s.timer);
    try { if (s.recorder?.state !== 'inactive') s.recorder.stop(); } catch { /* already stopped */ }
    for (const track of s.stream?.getTracks() || []) track.stop();
  }

  /**
   * Record the screen, or the window or tab the person picks in the browser's own dialog, until
   * `stopped` resolves, they end the share, or the longest recording is reached. `surface` is
   * what the dialog offers first. Resolves to { blob, durationMs }.
   */
  async function capture(surface, onTick, stopped) {
    const stream = await navigator.mediaDevices.getDisplayMedia({
      video: { displaySurface: surface, frameRate: { ideal: CAPTURE.frameRate, max: CAPTURE.frameRate * 2 }, width: { max: CAPTURE.maxSide }, height: { max: CAPTURE.maxSide } },
      audio: false,
      monitorTypeSurfaces: 'include',
      selfBrowserSurface: 'exclude',
    });
    const mimeType = MIME_TYPES.find((t) => MediaRecorder.isTypeSupported(t));
    const recorder = new MediaRecorder(stream, { ...(mimeType ? { mimeType } : {}), videoBitsPerSecond: CAPTURE.bitsPerSecond });
    const chunks = [];
    const startedAt = Date.now();
    const maxMs = (info.limits.recording_seconds || 600) * 1000;
    const mine = { stream, recorder, timer: null };
    session = mine;
    return new Promise((resolve, reject) => {
      const stop = () => { if (recorder.state !== 'inactive') recorder.stop(); };
      recorder.addEventListener('dataavailable', (e) => { if (e.data?.size) chunks.push(e.data); });
      recorder.addEventListener('error', (e) => reject(e.error || new Error(tr('actions.rec.failed'))));
      recorder.addEventListener('stop', () => {
        clearInterval(mine.timer);
        for (const track of stream.getTracks()) track.stop();
        const cancelled = session !== mine;
        if (!cancelled) session = null;
        const type = (recorder.mimeType || mimeType || 'video/webm').split(';')[0];
        if (cancelled) reject(Object.assign(new Error('cancelled'), { cancelled: true }));
        else resolve({ blob: new Blob(chunks, { type }), durationMs: Date.now() - startedAt });
      });
      // The browser's own "Stop sharing" ends the recording too.
      for (const track of stream.getVideoTracks()) track.addEventListener('ended', stop);
      stopped.then(stop);
      mine.timer = setInterval(() => {
        const elapsed = Date.now() - startedAt;
        onTick(elapsed, maxMs);
        if (elapsed >= maxMs) stop();
      }, 500);
      recorder.start(1000);
      onTick(0, maxMs);
    });
  }

  // -- making a recording small ------------------------------------------------------------------

  /**
   * How many cells of two signatures differ (see FRAME). A signature is a small picture's RGBA
   * bytes; a cell differs when any of its colours does, so two screens that are equally bright
   * but not the same colour still count as two.
   */
  function difference(a, b) {
    let cells = 0;
    for (let i = 0; i < a.length; i += 4) {
      if (Math.abs(a[i] - b[i]) > FRAME.cell || Math.abs(a[i + 1] - b[i + 1]) > FRAME.cell || Math.abs(a[i + 2] - b[i + 2]) > FRAME.cell) cells++;
    }
    return cells;
  }

  /** Until the page is in front: a video in a background tab isn't played or drawn reliably. */
  function visible() {
    if (document.visibilityState === 'visible') return Promise.resolve();
    return new Promise((resolve) => {
      const on = () => { if (document.visibilityState === 'visible') { document.removeEventListener('visibilitychange', on); resolve(); } };
      document.addEventListener('visibilitychange', on);
    });
  }

  /**
   * The frames of `blob` (a video the browser can play) in which the screen changed, as
   * [{ at_ms, media_type, data }] with base64 JPEGs, at most `maxFrames`. The video is played
   * through once, fast and muted. A new picture is kept once it has stood still for a moment,
   * so no frame shows a page half drawn; and the frame just before a change is kept if anything
   * small had happened in it (what was typed, where the pointer was when it clicked). Rejects if
   * the video can't be played.
   */
  async function keyFrames(blob, { maxFrames, maxBytes, onProgress, durationMs }) {
    await visible();
    const video = el('video');
    video.muted = true;
    video.playsInline = true;
    video.preload = 'auto';
    const url = URL.createObjectURL(blob);
    try {
      await new Promise((resolve, reject) => {
        video.addEventListener('loadeddata', resolve, { once: true });
        video.addEventListener('error', () => reject(new Error(tr('actions.rec.unplayable'))), { once: true });
        video.src = url;
      });
      const side = Math.max(video.videoWidth, video.videoHeight);
      if (!side) throw new Error(tr('actions.rec.unplayable'));
      const scale = Math.min(1, FRAME.maxSide / side);
      const size = [Math.max(1, Math.round(video.videoWidth * scale)), Math.max(1, Math.round(video.videoHeight * scale))];
      const canvas = () => Object.assign(el('canvas'), { width: size[0], height: size[1] });
      let cur = { canvas: canvas(), sig: null, at: 0 };
      let prev = { canvas: canvas(), sig: null, at: 0 };
      const small = Object.assign(el('canvas'), { width: FRAME.sigW, height: FRAME.sigH });
      const smallCtx = small.getContext('2d', { willReadFrequently: true });
      const frames = [];
      let kept = null;      // the signature of the last frame kept
      let since = 0;        // when the picture before the previous sample's was last seen, in the video's time
      let before = false;   // the frame before the change under way has been kept
      let changing = null;  // when the change under way began
      let lastAt = -Infinity;
      const keep = (frame, score) => {
        const data = frame.canvas.toDataURL('image/jpeg', FRAME.quality).split(',')[1] || '';
        if (data) frames.push({ at_ms: Math.round(frame.at), media_type: 'image/jpeg', data, score });
      };
      const sample = (mediaTime) => {
        const at = (Number.isFinite(mediaTime) ? mediaTime : video.currentTime) * 1000;
        if (at - lastAt < FRAME.gapMs) return;
        lastAt = at;
        cur.canvas.getContext('2d').drawImage(video, 0, 0, size[0], size[1]);
        smallCtx.drawImage(cur.canvas, 0, 0, FRAME.sigW, FRAME.sigH);
        const sig = smallCtx.getImageData(0, 0, FRAME.sigW, FRAME.sigH).data;
        cur.sig = sig;
        cur.at = at;
        if (!kept) {
          keep(cur, Infinity);
          kept = sig;
          since = at;
        } else {
          const change = difference(sig, kept);
          // How long the previous picture stood: from the last sight of the one before it until
          // now, since playing fast may show only one frame of it. (A blinking caret is no new
          // picture.)
          if (difference(sig, prev.sig) > 1) {
            const was = difference(prev.sig, kept);
            if (was >= FRAME.changed && at - since >= FRAME.settleMs) {
              // The previous picture was new and stood still: a state of the screen.
              keep(prev, was);
              kept = prev.sig;
              before = false;
              changing = null;
            } else if (!before && was > FRAME.settled && was < FRAME.changed && change >= FRAME.changed) {
              // Something small had happened (text typed, the pointer on a button) and now the
              // screen changes: what it looked like right before.
              keep(prev, FRAME.changed + was);
              before = true;
            }
            since = prev.at;
          }
          // A change that never comes to rest (something playing on the page) is kept anyway.
          if (difference(sig, kept) < FRAME.changed) {
            changing = null;
          } else {
            changing ??= at;
            if (at - changing > FRAME.restlessMs) {
              keep(cur, change);
              kept = sig;
              changing = null;
              before = false;
            }
          }
        }
        [cur, prev] = [prev, cur];
        if (durationMs) onProgress?.(Math.min(1, at / durationMs));
      };
      await new Promise((resolve, reject) => {
        let done = false;
        let watchdog = null;
        let stalledAt = -1;
        let stalledFor = 0;
        const tick = video.requestVideoFrameCallback ? 500 : 40;
        const finish = () => {
          if (done) return;
          done = true;
          clearInterval(watchdog);
          video.pause();
          resolve();
        };
        video.addEventListener('ended', finish, { once: true });
        video.addEventListener('error', () => (frames.length ? finish() : reject(new Error(tr('actions.rec.unplayable')))), { once: true });
        const onFrame = (_now, frame) => { if (done) return; sample(frame?.mediaTime); video.requestVideoFrameCallback(onFrame); };
        if (video.requestVideoFrameCallback) video.requestVideoFrameCallback(onFrame);
        // Without per-frame callbacks the timer samples; with them it only notices a stall.
        watchdog = setInterval(() => {
          if (!video.requestVideoFrameCallback) sample();
          if (video.currentTime !== stalledAt) { stalledAt = video.currentTime; stalledFor = 0; return; }
          // No progress: over, or stuck (a file that ends without saying so).
          stalledFor += tick;
          if (video.ended || video.paused || stalledFor >= 3000) finish();
        }, tick);
        for (const rate of PLAYBACK_RATES) {
          try { video.playbackRate = rate; break; } catch { /* this browser plays slower */ }
        }
        video.play().catch(reject);
      });
      // What the screen ended on.
      if (kept && prev.sig && difference(prev.sig, kept) > FRAME.settled) keep(prev, Infinity);
      onProgress?.(1);
      return thin(frames, maxFrames, maxBytes);
    } finally {
      video.removeAttribute('src');
      video.load();
      URL.revokeObjectURL(url);
    }
  }

  /** At most `maxFrames` frames and `maxBytes` in all: those that differ least go first, the first and last never. */
  function thin(frames, maxFrames, maxBytes) {
    const list = [...frames];
    const total = () => list.reduce((n, f) => n + base64Bytes(f.data), 0);
    while (list.length > 2 && (list.length > maxFrames || total() > maxBytes)) {
      let drop = 1;
      for (let i = 2; i < list.length - 1; i++) if (list[i].score < list[drop].score) drop = i;
      list.splice(drop, 1);
    }
    return list.map(({ at_ms, media_type, data }) => ({ at_ms, media_type, data }));
  }

  function base64Of(blob) {
    return new Promise((resolve, reject) => {
      const reader = new FileReader();
      reader.addEventListener('load', () => resolve(String(reader.result).split(',')[1] || ''));
      reader.addEventListener('error', () => reject(reader.error));
      reader.readAsDataURL(blob);
    });
  }

  /** Whether the video itself goes to the server: a model there takes video, and it is small enough and of a type it takes. */
  function sendsVideo(blob) {
    const types = info.recording.video_types || ['video/webm', 'video/mp4'];
    return !!info.recording.video && types.includes(blob.type) && blob.size <= (info.limits.video_bytes || 16 << 20);
  }

  // -- a new action: record, look, let Cedar write the steps, correct, save --------------------------

  function viewNew(goal) {
    const current = view;
    crumb();
    const body = page.body;
    const stage = el('div', 'cedar-act-stage');
    body.replaceChildren(heading(tr('actions.new.title')), el('p', 'cedar-ag-intro', tr('actions.new.intro')), stage);
    const alive = () => current === view;
    let recording = null; // { blob, durationMs, frames }
    let url = null;

    const fail = (message, retry) => {
      const box = el('div', 'cedar-ag-error', message);
      const actions = el('div', 'cedar-ag-form-actions');
      actions.append(button(tr('actions.new.startOver'), () => setup()), ...(retry ? [button(tr('actions.new.tryAgain'), retry, 'primary')] : []));
      stage.replaceChildren(box, actions);
    };

    // 1. What is about to be shown, and what to share. `notice` says why the person is back here.
    function setup(notice) {
      if (url) { URL.revokeObjectURL(url); url = null; }
      recording = null;
      const what = el('textarea', 'cedar-act-goal');
      what.rows = 2;
      what.maxLength = 1000;
      what.value = goal;
      what.placeholder = tr('actions.new.goalPlaceholder');
      what.addEventListener('input', () => { goal = what.value; });
      const picks = el('div', 'cedar-ag-picks');
      let surface = 'window';
      for (const [id, glyph] of [['window', 'window'], ['monitor', 'screen']]) {
        const pick = el('label', 'cedar-ag-pick');
        const radio = el('input');
        radio.type = 'radio';
        radio.name = 'cedar-act-surface';
        radio.checked = id === surface;
        radio.addEventListener('change', () => { surface = id; });
        pick.append(radio, icon(glyph), el('b', '', tr(`actions.new.surface.${id}`)), el('small', '', tr(`actions.new.surfaceHelp.${id}`)));
        picks.append(pick);
      }
      const file = el('input');
      file.type = 'file';
      file.accept = 'video/*';
      file.hidden = true;
      file.addEventListener('change', () => { if (file.files?.[0]) reduce(file.files[0], 0); });
      const start = button(tr('actions.new.start'), () => record(surface), 'primary');
      const upload = button(tr('actions.new.upload'), () => file.click());
      const blocked = !info.supported ? tr('actions.unsupported') : cannotWatch();
      start.disabled = upload.disabled = !!blocked;
      const actions = el('div', 'cedar-ag-form-actions');
      actions.append(link(tr('actions.cancel'), '#/actions', 'copy-btn'), upload, start);
      const form = el('div', 'cedar-ag-form');
      form.append(field(tr('actions.new.goal'), what, tr('actions.new.goalHelp')));
      if (blocked) form.append(el('div', 'cedar-ag-box warn cedar-act-notice', blocked));
      if (typeof notice === 'string') form.append(el('div', 'cedar-ag-box warn cedar-act-notice', notice));
      if (canRecord()) form.append(el('div', 'cedar-ag-legend', tr('actions.new.share')), picks);
      else { start.hidden = true; upload.className = 'primary'; form.append(el('div', 'cedar-ag-box cedar-act-notice', tr('actions.new.cannotRecord'))); }
      const notes = el('ul', 'cedar-act-notes');
      for (const key of ['small', 'once', 'passwords']) notes.append(el('li', '', tr(`actions.new.note.${key}`)));
      form.append(notes, file, actions);
      stage.replaceChildren(form);
    }

    // 2. Recording.
    async function record(surface) {
      let release;
      const stopped = new Promise((resolve) => { release = resolve; });
      const time = el('span', 'cedar-act-time', '0:00');
      const live = el('div', 'cedar-act-live');
      live.append(el('span', 'cedar-act-dot'), el('b', '', tr('actions.rec.recording')), time);
      const actions = el('div', 'cedar-ag-form-actions');
      actions.append(button(tr('actions.rec.stop'), () => release(), 'primary'));
      const box = el('div', 'cedar-ag-box');
      box.append(live, el('div', 'cedar-ag-note', tr('actions.rec.hint')), actions);
      let got;
      try {
        got = await capture(surface, (elapsed, max) => {
          if (!alive()) return;
          if (!box.isConnected) stage.replaceChildren(box);
          time.textContent = tr('actions.rec.time', { elapsed: clock(elapsed), max: clock(max) });
        }, stopped);
      } catch (e) {
        if (!alive() || e?.cancelled) return;
        // The person closed the browser's dialog without choosing, or the browser (an app's
        // embedded one, say) shares no screen and never asked: back to the start, with a way on.
        if (e?.name === 'NotAllowedError' || e?.name === 'AbortError') { setup(tr('actions.rec.notShared')); return; }
        fail(e?.message || tr('actions.rec.failed'));
        return;
      }
      if (!alive()) return;
      if (!got.blob.size) { fail(tr('actions.rec.empty')); return; }
      reduce(got.blob, got.durationMs);
    }

    // 3. Making it small: the frames in which something changed.
    async function reduce(blob, durationMs) {
      const bar = el('progress', 'cedar-act-progress');
      bar.max = 1;
      const box = el('div', 'cedar-ag-box');
      box.append(el('b', '', tr('actions.reduce.title')), el('div', 'cedar-ag-note', tr('actions.reduce.note')), bar);
      stage.replaceChildren(box);
      try {
        const frames = await keyFrames(blob, {
          maxFrames: Math.min(info.limits.frames || 40, 40),
          maxBytes: info.limits.frames_total_bytes || 12 << 20,
          durationMs,
          onProgress: (p) => { bar.value = p; },
        });
        if (!alive()) return;
        if (!frames.length) { fail(tr('actions.rec.unplayable')); return; }
        recording = { blob, durationMs: durationMs || frames[frames.length - 1].at_ms, frames };
        look();
      } catch (e) {
        if (alive()) fail(e?.message || tr('actions.rec.unplayable'));
      }
    }

    // 4. A look at what was recorded and what of it will be sent.
    function look() {
      const { blob, durationMs, frames } = recording;
      url ||= URL.createObjectURL(blob);
      const video = el('video', 'cedar-act-video');
      video.controls = true;
      video.muted = true;
      video.playsInline = true;
      video.src = url;
      const strip = el('div', 'cedar-act-strip');
      for (const f of frames) {
        const img = el('img');
        img.src = `data:${f.media_type};base64,${f.data}`;
        img.alt = '';
        img.title = clock(f.at_ms);
        img.loading = 'lazy';
        strip.append(img);
      }
      const asVideo = sendsVideo(blob);
      const frameBytes = frames.reduce((n, f) => n + base64Bytes(f.data), 0);
      const facts = el('div', 'cedar-ag-meta');
      facts.append(
        el('span', '', tr('actions.look.length', { length: clock(durationMs) })),
        el('span', '', tr('actions.look.video', { size: megabytes(blob.size) })),
        el('span', '', tr('actions.look.frames', { count: frames.length, size: megabytes(frameBytes) })),
      );
      const sends = el('div', 'cedar-ag-note', asVideo ? tr('actions.look.sendsVideo', { size: megabytes(blob.size) }) : tr('actions.look.sendsFrames', { count: frames.length }));
      const actions = el('div', 'cedar-ag-form-actions');
      actions.append(link(tr('actions.cancel'), '#/actions', 'copy-btn'), button(tr('actions.look.again'), () => setup()), button(tr('actions.look.write'), () => write(asVideo), 'primary'));
      const box = el('div', 'cedar-ag-box');
      box.append(el('b', '', tr('actions.look.title')), video, facts, el('div', 'cedar-act-strip-label', tr('actions.look.kept')), strip, sends);
      stage.replaceChildren(box, actions);
    }

    // 5. Cedar's model writes the steps.
    async function write(asVideo) {
      const busy = el('div', 'cedar-ag-review');
      busy.append(el('span', 'spinner'), el('span', '', tr('actions.write.busy')));
      stage.replaceChildren(busy);
      try {
        const { blob, durationMs, frames } = recording;
        const body = { goal: goal.trim(), duration_ms: Math.round(durationMs), locale: cedarI18n.locale };
        if (asVideo) body.video = { media_type: blob.type, data: await base64Of(blob) };
        else body.frames = frames;
        const result = await deps.api('POST', '/actions/draft', body);
        if (!alive()) return;
        const draft = result.draft || {};
        const source = { kind: 'recording', seconds: Math.round(durationMs / 1000), frames: frames.length, watched: result.watched, model: result.model_id };
        edit({ title: draft.title, summary: draft.summary, site: draft.site, steps: draft.steps || [], secrets: draft.secrets || [] }, { source, notes: draft.notes, model: result.model });
      } catch (e) {
        if (alive()) fail(e?.message || tr('actions.write.failed'), () => write(asVideo));
      }
    }

    // 6. The person reads, corrects and saves.
    function edit(action, extra) {
      stage.replaceChildren(form(action, {
        ...extra,
        cancel: () => look(),
        cancelLabel: tr('actions.form.backToRecording'),
        save: async (body) => {
          const saved = await deps.api('POST', '/actions', body);
          if (url) { URL.revokeObjectURL(url); url = null; }
          deps.toast(tr('actions.saved'));
          await load();
          location.hash = `#/actions/${saved.id}`;
        },
      }));
    }

    // What this server can do with a recording decides what the first step offers; when that
    // isn't known yet, the step is drawn again once it is, unless the person has moved on.
    const known = started;
    setup();
    const first = stage.firstElementChild;
    if (!known) load().then(() => { if (alive() && stage.firstElementChild === first) setup(); });
  }

  // -- the form: an action's text and the passwords stored with it ------------------------------------

  /**
   * The form for `action` ({title, summary, site, steps, secrets}). `save(body)` saves what it
   * holds and may throw; `notes` and `model` say what the model made of a recording.
   */
  function form(action, { save, cancel, cancelLabel, notes, model, source, remove } = {}) {
    const f = el('form', 'cedar-ag-form');
    const title = input(action.title, { max: 120 });
    title.required = true;
    const summary = input(action.summary, { max: 400 });
    const site = input(action.site, { max: 253, placeholder: 'portal.example.com' });
    const steps = el('textarea', 'cedar-act-steps');
    steps.value = (action.steps || []).join('\n');
    steps.rows = Math.min(24, Math.max(6, (action.steps || []).length + 2));
    steps.required = true;
    const error = el('div', 'cedar-ag-error');
    error.hidden = true;

    if (model) f.append(el('div', 'cedar-ag-note', tr('actions.form.written', { model })));
    if (notes) {
      const box = el('div', 'cedar-ag-box warn');
      box.append(el('b', '', tr('actions.form.notes')), el('div', '', notes));
      f.append(box);
    }
    f.append(
      field(tr('actions.form.title'), title),
      field(tr('actions.form.summary'), summary, tr('actions.form.summaryHelp')),
      field(tr('actions.form.steps'), steps, tr('actions.form.stepsHelp')),
      field(tr('actions.form.site'), site, tr('actions.form.siteHelp')),
    );
    if (action.site && !info.browser) f.append(el('div', 'cedar-ag-box warn', tr('actions.form.noBrowser')));

    // Passwords: what the recording showed being typed, and any the person adds.
    const set = el('fieldset', 'cedar-ag-set');
    const rows = el('div', 'cedar-act-secrets');
    const entries = [];
    const freeName = () => {
      const taken = new Set(entries.map((e) => e.name));
      if (!taken.has('password')) return 'password';
      for (let n = 2; ; n++) if (!taken.has(`password_${n}`)) return `password_${n}`;
    };
    const addSecret = (s) => {
      const entry = { name: s.name || freeName(), saved: !!s.saved };
      const label = input(s.label, { max: 80, placeholder: tr('actions.secret.labelPlaceholder') });
      const value = input('', { type: 'password', max: 1024, placeholder: entry.saved ? tr('actions.secret.keep') : tr('actions.secret.value') });
      value.autocomplete = 'new-password';
      value.disabled = !info.secrets;
      entry.label = label;
      entry.value = value;
      const row = el('div', 'cedar-act-secret');
      const head = el('div', 'cedar-act-secret-head');
      head.append(icon('lock'), el('code', '', entry.name), el('span', entry.saved ? 'cedar-ag-chip ok' : 'cedar-ag-chip', tr(entry.saved ? 'actions.secret.stored' : 'actions.secret.notStored')));
      head.append(button(tr('actions.secret.remove'), () => { entries.splice(entries.indexOf(entry), 1); row.remove(); }));
      const pair = el('div', 'cedar-ag-row');
      pair.append(field(tr('actions.secret.label'), label), field(tr('actions.secret.password'), value));
      row.append(head, pair);
      rows.append(row);
      entries.push(entry);
    };
    for (const s of action.secrets || []) addSecret(s);
    const add = button(tr('actions.secret.add'), () => addSecret({}));
    set.append(el('div', 'cedar-ag-legend', tr('actions.secret.title')), el('div', 'cedar-ag-note', tr(info.secrets ? 'actions.secret.note' : 'actions.secret.noStorage')), rows, el('div', 'cedar-ag-inline', ''));
    set.lastChild.append(add);
    f.append(set, error);

    const submit = el('button', 'primary', tr('actions.form.save'));
    submit.type = 'submit';
    const actions = el('div', 'cedar-ag-form-actions');
    if (remove) actions.append(button(tr('actions.delete'), remove, 'copy-btn cedar-act-danger'));
    if (cancel) actions.append(button(cancelLabel || tr('actions.cancel'), cancel));
    actions.append(submit);
    f.append(actions);

    f.addEventListener('submit', async (e) => {
      e.preventDefault();
      const body = {
        title: title.value,
        summary: summary.value,
        site: site.value.trim() || null,
        steps: steps.value.split('\n').map((s) => s.replace(/^\s*(?:\d+[.)]|[-*•])\s+/, '').trim()).filter(Boolean),
        secrets: entries.map((s) => ({ name: s.name, label: s.label.value.trim(), ...(s.value.value ? { value: s.value.value } : {}) })),
        ...(source ? { source } : {}),
      };
      error.hidden = true;
      if (!body.site && entries.some((s) => s.value.value)) {
        error.textContent = tr('actions.secret.needsSite');
        error.hidden = false;
        site.focus();
        return;
      }
      submit.disabled = true;
      try {
        await save(body);
      } catch (err) {
        error.textContent = err?.message || tr('actions.saveFailed');
        error.hidden = false;
      } finally {
        submit.disabled = false;
        for (const s of entries) s.value.value = '';
      }
    });
    return f;
  }

  // -- one action --------------------------------------------------------------------------------

  async function viewAction(id) {
    const current = view;
    crumb();
    page.body.replaceChildren(el('p', 'cedar-ag-note', tr('actions.loading')));
    let action;
    try {
      await load();
      action = await deps.api('GET', `/actions/${id}`);
    } catch {
      if (current === view) page.body.replaceChildren(heading(tr('actions.title')), el('div', 'cedar-ag-error', tr('actions.gone')));
      return;
    }
    if (current !== view) return;
    const draw = (a) => {
      const meta = el('div', 'cedar-ag-meta');
      meta.append(el('code', '', a.name), el('span', '', a.uses ? tr('actions.used', { count: a.uses }) : tr('actions.unused')));
      if (a.source?.kind === 'recording') meta.append(el('span', '', tr('actions.fromRecording', { length: clock((a.source.seconds || 0) * 1000) })));
      const edit = form(a, {
        save: async (body) => {
          const saved = await deps.api('PUT', `/actions/${a.id}`, body);
          deps.toast(tr('actions.saved'));
          load();
          if (current === view) draw(saved);
        },
        remove: async () => {
          if (!await cedarConfirm(tr('actions.deleteConfirm', { title: a.title }), { ok: tr('actions.delete') })) return;
          try {
            await deps.api('DELETE', `/actions/${a.id}`);
            await load();
            location.hash = '#/actions';
          } catch (e) { deps.toast(e?.message || tr('actions.saveFailed')); }
        },
      });
      page.body.replaceChildren(heading(a.title), meta, el('p', 'cedar-ag-intro', tr('actions.detail.intro')), edit);
    };
    draw(action);
  }

  // -- wiring ------------------------------------------------------------------------------------

  function bind() {
    // Leaving with Escape, as a dialog would be left, unless one is open on top, a field has the
    // focus, or a recording is being made.
    document.addEventListener('keydown', (e) => {
      if (e.key !== 'Escape' || !page || page.root.hidden || session || document.querySelector('dialog[open]')) return;
      if (/^(INPUT|TEXTAREA|SELECT)$/.test(document.activeElement?.tagName || '')) return;
      leave();
    });
  }

  function init(helpers) {
    deps = helpers;
    if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', bind);
    else bind();
  }

  /** Load the actions for the sidebar link; once, after the access token is known. */
  async function start() {
    if (started || !deps) return;
    await load();
    started = true;
  }

  /** Open the recorder to show how `goal` is done (the card of something Cedar couldn't do). */
  function show_(goal) {
    location.hash = `#/actions/new${goal ? `?goal=${encodeURIComponent(goal)}` : ''}`;
  }

  return { init, start, route, record: show_, keyFrames, thin };
})();

// The setup page: polls the app for its status and sends the model server choice back.
// `window.__TAURI__` is there because tauri.conf.json sets app.withGlobalTauri.
'use strict';

const invoke = (cmd, args) => window.__TAURI__.core.invoke(cmd, args || {});
const $ = (id) => document.getElementById(id);

let lastStatus = null;
let formTouched = false;
// The Model server step's option whose form is open ('llama', 'own', 'hosted' or none), and
// whether the user opened or closed one themselves; until then it follows the saved choice.
let openSource = null;
let sourceTouched = false;
let hostedTouched = false;

// The model servers people run themselves, for the own-server form: where each listens by
// default, and the key of its hint in strings.js. From the app (chat/static/model-servers.json,
// which the chat's Settings uses too); the buttons are built when the first status arrives.
let PRESETS = [];
let shareBusy = false;

function dot(el, cls) {
  el.className = 'dot' + (cls ? ' ' + cls : '');
}

// The page's wording for the platform the app runs on (macOS or Windows), in strings.js under
// platform.<os>.<field>: computer, computerSubject, dockerApps, dockerStart, dockerStarting,
// setupIntro, tailscaleIntro, tailscaleNext.
function platform(os) {
  const name = os === 'windows' ? 'windows' : 'macos';
  return {
    text: (field) => tr(`platform.${name}.${field}`),
    // Only offered when no Docker is found on a Mac: one that won't start may be OrbStack or Colima.
    setupWhenNotRunning: name === 'windows',
  };
}

/** A link that opens in the default browser (see the click handler below). */
function link(href, text) {
  const a = document.createElement('a');
  a.href = href;
  a.target = '_blank';
  a.rel = 'noreferrer';
  a.textContent = text;
  return a;
}

function render(s) {
  lastStatus = s;
  const p = platform(s.platform);
  for (const e of document.querySelectorAll('.this-computer')) e.textContent = p.text('computer');
  $('version').textContent = 'v' + s.version;
  $('config-path').textContent = s.config_path;
  $('sandbox-image').textContent = s.sandbox_image;

  // Docker
  const d = s.docker;
  const dockerHelp = $('docker-help');
  dockerHelp.classList.add('hidden');
  if (d.state === 'checking') {
    dot($('docker-dot'), 'busy');
    $('docker-text').textContent = tr('docker.looking');
  } else if (d.state === 'missing') {
    dot($('docker-dot'), 'bad');
    // When Cedar can set Docker up itself, that offer leads and installing it by hand moves below
    // it (see renderDockerSetup).
    $('docker-text').textContent = s.docker_setup_available
      ? tr('docker.missingSetup')
      : tr('docker.missing', { apps: p.text('dockerApps') });
    dockerHelp.href = 'https://www.docker.com/products/docker-desktop/';
    dockerHelp.classList.remove('hidden');
  } else if (d.state === 'starting') {
    dot($('docker-dot'), 'busy');
    $('docker-text').textContent = tr('docker.firstStart', { starting: p.text('dockerStarting') });
  } else if (d.state === 'not_running') {
    dot($('docker-dot'), 'warn');
    $('docker-text').textContent = tr('docker.notRunning', { start: p.text('dockerStart'), detail: d.detail });
  } else {
    dot($('docker-dot'), 'ok');
    $('docker-text').textContent = d.detail;
  }

  renderDockerSetup(s, p);

  // Images
  const i = s.images;
  $('images-detail').textContent = '';
  $('images-progress-text').textContent = '';
  showProgress($('images-progress'), i.state === 'pulling' ? pullProgress(i.detail) : null);
  if (i.state === 'waiting') {
    dot($('images-dot'), '');
    $('images-text').textContent = tr('images.waiting');
  } else if (i.state === 'pulling') {
    dot($('images-dot'), 'busy');
    $('images-text').textContent = tr('images.pulling');
    $('images-progress-text').textContent = pullText(i.detail);
    $('images-detail').textContent = i.detail.text;
  } else if (i.state === 'failed') {
    dot($('images-dot'), 'bad');
    $('images-text').textContent = tr('images.failed');
    $('images-detail').textContent = i.detail;
  } else {
    dot($('images-dot'), 'ok');
    $('images-text').textContent = tr('images.ready');
  }
  const toggle = $('browser-toggle');
  if (document.activeElement !== toggle) toggle.checked = s.browser;

  // The forms of the user's own server and of a hosted provider: each filled from the saved
  // config while that is the option in use, until the user starts typing.
  if (!formTouched) {
    const own = s.model_source === 'own';
    $('base-url').value = own ? s.llm_base_url || '' : '';
    $('api-key').placeholder = own && s.has_api_key ? tr('model.saved') : '';
  }
  renderPreset();
  renderHosted(s);
  renderModelChoice(s, p);
  const m = s.model;
  // What the line names: the model server, or with none, the model the app runs with llama.cpp.
  const name = s.llm_model.trim() || s.llama.model || '';
  const using = s.llm_model.trim()
    ? tr('model.using', { model: s.llm_model.trim(), url: s.llm_base_url.trim() })
    : tr('llama.running', { model: name, computer: p.text('computer') });
  $('model-detail').textContent = '';
  if (m.state === 'registering') {
    dot($('model-dot'), 'busy');
    $('model-text').textContent = tr('model.registering', { model: name });
  } else if (m.state === 'registered') {
    dot($('model-dot'), 'ok');
    // m.detail says the same in English; the page words it in the user's language.
    $('model-text').textContent = using;
  } else if (m.state === 'failed') {
    dot($('model-dot'), 'bad');
    $('model-text').textContent = tr('model.failed', { model: name });
    $('model-detail').textContent = m.detail;
  } else if (s.configured) {
    dot($('model-dot'), 'warn');
    $('model-text').textContent = s.llm_model.trim() ? tr('model.pending', { model: s.llm_model, url: s.llm_base_url }) : using;
  } else {
    dot($('model-dot'), 'warn');
    $('model-text').textContent = tr('model.notChosen');
  }

  renderLlama(s, p);

  // Services
  const sv = s.services;
  $('services-detail').textContent = '';
  const running = sv.state === 'running';
  if (sv.state === 'starting') {
    dot($('services-dot'), 'busy');
    $('services-text').textContent = tr('services.starting', { detail: sv.detail });
  } else if (sv.state === 'crashed') {
    dot($('services-dot'), 'bad');
    $('services-text').textContent = tr('services.crashed');
    $('services-detail').textContent = sv.detail;
  } else if (running) {
    dot($('services-dot'), 'ok');
    $('services-text').textContent = tr('services.running', { url: s.chat_url });
  } else {
    dot($('services-dot'), '');
    $('services-text').textContent = s.configured ? tr('services.notRunning') : tr('services.waiting');
  }
  // Sharing with the phone apps (iOS, Android)
  const sh = s.share;
  const shareToggle = $('share-toggle');
  if (document.activeElement !== shareToggle && !shareBusy) shareToggle.checked = s.tailscale_share;
  $('share-detail').textContent = '';
  $('share-actions').classList.toggle('hidden', sh.state !== 'on');
  if (sh.state !== 'on') hideQr();
  $('share-retry').classList.toggle('hidden', sh.state !== 'failed');
  if (sh.state === 'starting') {
    dot($('share-dot'), 'busy');
    $('share-text').textContent = tr('share.starting');
  } else if (sh.state === 'on') {
    dot($('share-dot'), 'ok');
    $('share-text').textContent = tr('share.on', { url: sh.detail, computer: p.text('computerSubject') });
  } else if (sh.state === 'failed') {
    dot($('share-dot'), 'bad');
    $('share-text').textContent = tr('share.failed');
    $('share-detail').textContent = sh.detail;
  } else if (s.tailscale_share) {
    dot($('share-dot'), 'warn');
    $('share-text').textContent = tr('share.pending');
  } else {
    dot($('share-dot'), '');
    $('share-text').textContent = tr('share.off', { computer: p.text('computerSubject') });
  }

  renderTailscaleSetup(s, p);

  $('open-chat').disabled = !running;
  $('open-browser').disabled = !running;

  renderRemote(s);
  renderUpdate(s);
  renderActivity(s, p);
}

// A chat server on another computer (src/remote.rs): offered, folded, while unused; while in use,
// its state replaces the sections about this computer's own servers.
function renderRemote(s) {
  const inUse = Boolean(s.remote_server);
  document.body.classList.toggle('remote', inUse);
  $('remote-offer').classList.toggle('hidden', inUse);
  $('remote-in-use').classList.toggle('hidden', !inUse);
  if (!inUse) return;
  const r = s.remote;
  $('remote-detail').textContent = '';
  if (r.state === 'connected') {
    dot($('remote-dot'), 'ok');
    $('remote-text').textContent = tr('remote.connected', { url: s.remote_server });
  } else if (r.state === 'failed') {
    dot($('remote-dot'), 'bad');
    $('remote-text').textContent = tr('remote.failed', { url: s.remote_server });
    $('remote-detail').textContent = r.detail;
  } else {
    dot($('remote-dot'), 'busy');
    $('remote-text').textContent = tr('remote.checking', { url: s.remote_server });
  }
  const ready = r.state === 'connected';
  $('remote-open').disabled = !ready;
  $('remote-browser').disabled = !ready;
  $('remote-retry').disabled = r.state === 'checking';
}

// The banner under the header: the one thing that is going on, from the whole status. Either
// Cedar is busy with a step of the launch sequence (see bootstrap in src/app.rs), or it waits on
// the user to do something, or it is done. The first thing that applies wins, in the order of the
// launch sequence, so the banner always names the step that holds everything else up.
function activity(s, p) {
  const busy = (text, target) => ({ mode: 'busy', text, target });
  const waiting = (text, target) => ({ mode: 'waiting', text, target });
  const d = s.docker;
  const ds = s.docker_setup;
  const i = s.images;
  const ts = s.tailscale_setup;
  const sh = s.share;
  const sv = s.services;
  const m = s.model;
  // A chat server on another computer: nothing else applies.
  if (s.remote_server) {
    const r = s.remote;
    if (r.state === 'connected') return { mode: 'ready', text: tr('activity.remoteReady', { url: s.remote_server }), target: 'remote' };
    if (r.state === 'failed') return waiting(tr('activity.remoteFailed', { url: s.remote_server }), 'remote');
    return busy(tr('remote.checking', { url: s.remote_server }), 'remote');
  }
  // Docker, and setting it up from here.
  if (ds.state === 'running') return busy(tr('activity.dockerSetup', { detail: ds.detail }), 'docker');
  if (ds.state === 'needs_restart') return waiting(tr('activity.restart'), 'docker');
  if (ds.state === 'needs_sign_out') return waiting(tr('activity.signOut'), 'docker');
  if (ds.state === 'needs_virtualization') return waiting(tr('activity.virtualization'), 'docker');
  if (d.state === 'checking') return busy(tr('docker.looking'), 'docker');
  if (d.state === 'starting') return busy(tr('docker.firstStart', { starting: p.text('dockerStarting') }), 'docker');
  if (ds.state === 'failed') return waiting(tr('activity.dockerSetupFailed'), 'docker');
  if (d.state === 'missing') {
    const key = s.docker_setup_available ? 'activity.dockerMissingSetup' : 'activity.dockerMissing';
    return waiting(tr(key, { apps: p.text('dockerApps') }), 'docker');
  }
  if (d.state === 'not_running') return waiting(tr('activity.dockerNotRunning', { start: p.text('dockerStart') }), 'docker');
  // The images.
  if (i.state === 'pulling') return { ...busy(tr('activity.pulling'), 'images'), progress: pullProgress(i.detail) };
  if (i.state === 'failed') return waiting(tr('activity.pullFailed'), 'images');
  // Installing Tailscale runs on its own; while it does, it is what the page waits for.
  if (ts.state === 'running') return busy(tr('activity.tsSetup', { detail: ts.detail }), 'share');
  // The model server (llama.cpp set up from here, or the user's own), the servers, and the model
  // in cedar's registry.
  const l = s.llama;
  if (l.setup.state === 'running') {
    const progress = typeof l.progress === 'number' ? l.progress : undefined;
    return { ...busy(tr('activity.llamaSetup', { detail: l.setup.detail }), 'model'), progress };
  }
  if (l.setup.state === 'failed') return waiting(tr('activity.llamaFailed'), 'model');
  if (!s.configured) {
    if (l.offer) return waiting(tr('activity.chooseModelOffer', { computer: p.text('computer') }), 'model');
    return waiting(tr('activity.chooseModel'), 'model');
  }
  if (sh.state === 'starting') return busy(tr('share.starting'), 'share');
  if (sv.state === 'starting') return busy(tr('activity.starting', { detail: sv.detail }), 'services');
  if (sv.state === 'crashed') return waiting(tr('activity.crashed'), 'services');
  if (l.model && l.server.state === 'starting') return busy(tr('activity.llamaLoading', { model: l.model }), 'model');
  if (l.model && l.server.state === 'crashed') return waiting(tr('activity.llamaCrashed'), 'model');
  if (m.state === 'registering') return busy(tr('activity.registering', { model: s.llm_model.trim() || l.model || '' }), 'model');
  if (sv.state !== 'running') return waiting(tr('activity.notRunning'), 'services');
  if (m.state === 'failed') return waiting(tr('activity.modelFailed', { model: s.llm_model.trim() || l.model || '' }), 'model');
  // Sharing with the phone apps, only once it was asked for.
  if (ts.state === 'installed' && sh.state !== 'on') return waiting(tr('activity.tsLogin'), 'share');
  if (ts.state === 'failed') return waiting(tr('activity.tsFailed'), 'share');
  if (sh.state === 'failed') return waiting(tr('activity.shareFailed'), 'share');
  return { mode: 'ready', text: tr('activity.ready'), target: 'services' };
}

function renderActivity(s, p) {
  const a = activity(s, p);
  const el = $('activity');
  el.className = 'activity ' + a.mode;
  el.dataset.target = a.target;
  const titles = { busy: 'activity.busy', waiting: 'activity.waiting', ready: 'activity.readyTitle' };
  $('activity-title').textContent = tr(titles[a.mode]);
  $('activity-text').textContent = a.text;
  $('activity-icon').textContent = a.mode === 'waiting' ? '!' : a.mode === 'ready' ? '✓' : '';
  $('activity-go').classList.toggle('hidden', a.mode === 'ready');
  showProgress($('activity-progress'), a.progress);
}

// How far an image download is, from 0 to 1 (worked out in src/pull.rs), or -1 until Docker
// has said which layers there are.
function pullProgress(detail) {
  return typeof detail.fraction === 'number' ? detail.fraction : -1;
}

// The megabytes so far (and in all, once Docker has said), then the layers.
function pullText(detail) {
  if (!detail.layers) return '';
  const mb = (bytes) => (bytes < 10e6 ? Math.round(bytes / 1e5) / 10 : Math.round(bytes / 1e6));
  const parts = [];
  if (detail.total_bytes) parts.push(tr('images.bytesOf', { done: mb(detail.bytes), total: mb(detail.total_bytes) }));
  else if (detail.bytes) parts.push(tr('images.bytes', { done: mb(detail.bytes) }));
  parts.push(tr('images.layers', detail));
  return parts.join(' ');
}

// Shows a progress bar at `value` (0 to 1), as a bar without a position for -1, or hides it for
// null or undefined.
function showProgress(el, value) {
  const shown = value !== null && value !== undefined;
  el.classList.toggle('hidden', !shown);
  if (shown && value >= 0) el.value = value;
  else el.removeAttribute('value');
}

// Clicking the banner (or pressing Enter on it) scrolls to the step it names.
function goToActivity() {
  const target = $('activity').dataset.target;
  if (target) $(target).scrollIntoView({ behavior: 'smooth', block: 'start' });
}
$('activity').addEventListener('click', goToActivity);
$('activity').addEventListener('keydown', (ev) => {
  if (ev.key === 'Enter' || ev.key === ' ') {
    ev.preventDefault();
    goToActivity();
  }
});

// A newer release (see src/update.rs): the banner at the top, and the Updates section.
function renderUpdate(s) {
  const u = s.update;
  const busy = u.state === 'downloading' || u.state === 'installing';
  const show = Boolean(u.version) && (busy || u.state === 'failed' || !u.dismissed);
  $('update').classList.toggle('hidden', !show);
  if (show) {
    $('update-title').textContent = tr(busy ? 'update.updatingTo' : 'update.available', { version: u.version });
    $('update-text').className = u.state === 'failed' ? 'error' : 'muted';
    $('update-text').textContent = busy
      ? (u.detail || tr('update.working'))
      : u.state === 'failed'
        ? tr('update.failed', { detail: u.detail })
        : tr('update.restartNote', { current: u.current });
    $('update-notes').href = u.notes_url || '#';
    $('update-install').textContent = u.state === 'failed' ? tr('common.tryAgain') : tr('update.install');
    $('update-install').disabled = busy;
    $('update-dismiss').classList.toggle('hidden', busy);
  }

  const toggle = $('updates-toggle');
  if (document.activeElement !== toggle) toggle.checked = s.check_updates;
  $('updates-check').disabled = !u.enabled || u.state === 'checking' || busy;
  const params = { current: u.current, version: u.version, detail: u.detail };
  let key;
  if (!u.enabled && s.check_updates) key = 'updates.noSelfUpdate';
  else if (!s.check_updates) key = 'updates.off';
  else if (u.state === 'checking') key = 'updates.checking';
  else if (u.version) key = 'updates.available';
  else if (u.detail) key = 'updates.failed';
  else if (u.up_to_date) key = 'updates.latest';
  else key = 'updates.idle';
  $('updates-text').textContent = tr(key, params);
}

// Setting Docker up from here (Windows and macOS): offered while Docker is missing (on Windows,
// also while it won't start), and shown until it has finished.
// The Model server step's three options (see index.html): running a model on this computer with
// llama.cpp, a hosted provider, or a server the user already runs. What is in use is tagged (the
// first can be in use beside one of the others) and a form is open, until the user opens another; a llama.cpp setup that is running or failed
// keeps its own open. The first is greyed out when no GPU here can run one of Cedar's models,
// and the provider is the recommended one then.
function renderModelChoice(s, p) {
  const l = s.llama;
  const settingUp = l.setup.state === 'running' || l.setup.state === 'failed';
  const canLlama = !!l.offer || !!l.model || settingUp;
  // What is in use is open: the model server's form, or with none, the llama.cpp one.
  if (!sourceTouched) openSource = settingUp ? 'llama' : s.model_source || (l.model ? 'llama' : null);
  if (openSource === 'llama' && !canLlama) openSource = null;

  $('pick-llama').disabled = !canLlama;
  $('pick-llama-text').textContent = canLlama
    ? tr('pick.llamaText', { computer: p.text('computer') })
    : tr('pick.llamaNo', { computer: p.text('computer') });
  for (const source of ['llama', 'own', 'hosted']) {
    const card = $(`pick-${source}`);
    const open = openSource === source;
    card.classList.toggle('open', open);
    card.setAttribute('aria-expanded', String(open));
    $(source).classList.toggle('hidden', !open);
    const tag = $(`pick-${source}-tag`);
    // A model the app runs and a model server can both be in use: cedar has an entry for each.
    const inUse = source === 'llama' ? !!l.model : s.model_source === source;
    // Until something is chosen: running a model here where the GPU can, a provider where not.
    const recommended = !s.configured && source === (l.offer ? 'llama' : 'hosted');
    tag.textContent = inUse ? tr('pick.inUse') : recommended ? tr('pick.recommended') : '';
    tag.classList.toggle('hidden', !inUse && !recommended);
  }
}

// The own-server form's presets: the one whose address is in the field is marked, with its hint
// under the row. Any other address has neither.
function renderPreset() {
  buildPresets();
  const address = $('base-url').value.trim().replace(/\/+$/, '');
  let hint = '';
  for (const button of document.querySelectorAll('#presets button')) {
    const preset = PRESETS[Number(button.dataset.preset)];
    const chosen = preset.url === address;
    button.classList.toggle('chosen', chosen);
    button.setAttribute('aria-pressed', String(chosen));
    if (chosen) hint = tr(preset.hint);
  }
  $('preset-hint').textContent = hint;
}

// The hosted provider's form: the list of providers (from src/models.rs, with "Other" for one
// that is not on it), the link to the chosen one's documentation, and, while a provider is the
// option in use, the saved one selected.
function renderHosted(s) {
  const select = $('hosted-provider');
  if (!select.options.length) {
    for (const provider of s.providers) select.add(new Option(provider.name, provider.base_url));
    select.add(new Option(tr('hosted.other'), ''));
  }
  select.options[select.options.length - 1].textContent = tr('hosted.other');
  if (!hostedTouched) {
    const hosted = s.model_source === 'hosted';
    const known = hosted && s.providers.some((provider) => provider.base_url === s.llm_base_url);
    select.value = !hosted || known ? (hosted ? s.llm_base_url : s.providers[0].base_url) : '';
    $('hosted-url').value = hosted && !known ? s.llm_base_url : '';
    $('hosted-key').placeholder = hosted && s.has_api_key ? tr('model.saved') : '';
  }
  const provider = s.providers.find((item) => item.base_url === select.value);
  $('hosted-other').classList.toggle('hidden', !!provider);
  const docs = $('hosted-docs');
  if (provider) docs.replaceChildren(link(provider.docs, tr('hosted.docs', { provider: provider.name })));
  else docs.replaceChildren();
}

// How far a llama.cpp download is for a progress bar: 0 to 1, or -1 for a step without a
// position (checking, unpacking).
function llamaProgress(l) {
  return typeof l.progress === 'number' ? l.progress : -1;
}

// llama.cpp as the model server, set up and run by the app (src/llamasetup.rs): offered when this
// computer's GPU can run one of Cedar's models, with the download's progress while it is set up,
// and what it runs once it is.
function renderLlama(s, p) {
  const l = s.llama;
  const st = l.setup;
  const offer = l.offer;
  const installed = !!l.model;
  const running = st.state === 'running';
  const failed = st.state === 'failed';

  const gb = (bytes) => Math.round(bytes / 1e9);
  const intro = $('llama-intro');
  if (installed) {
    intro.textContent = tr('llama.running', { model: l.model, computer: p.text('computer') });
  } else if (offer) {
    intro.textContent = tr(offer.unified ? 'llama.offerUnified' : 'llama.offer', {
      computer: p.text('computer'),
      gpu: offer.gpu,
      memory: Math.round(offer.memory_mb / 1024),
      model: offer.model,
      size: gb(offer.download_bytes),
    });
  } else {
    intro.textContent = '';
  }

  const install = $('llama-install');
  // Once installed there is nothing to set up, unless a newer llama.cpp could not be fetched.
  install.classList.toggle('hidden', installed && !failed);
  install.disabled = running;
  install.textContent = failed ? tr('common.tryAgain') : tr('llama.install');
  install.dataset.command = installed ? 'recheck' : 'install_llama';
  $('llama-cancel').classList.toggle('hidden', !(running && typeof l.progress === 'number'));
  $('llama-remove').classList.toggle('hidden', !installed || running);
  showProgress($('llama-progress'), running ? llamaProgress(l) : null);

  const text = $('llama-text');
  const detail = $('llama-detail');
  text.className = 'muted';
  text.textContent = '';
  detail.textContent = '';
  if (running) {
    text.textContent = st.detail;
  } else if (failed) {
    text.className = 'error';
    text.textContent = tr('llama.failed');
    detail.textContent = st.detail;
  } else if (installed && l.server.state === 'starting') {
    text.textContent = tr('llama.loading', { model: l.model });
  } else if (installed && l.server.state === 'crashed') {
    text.className = 'error';
    text.textContent = tr('llama.crashed');
    detail.textContent = l.server.detail;
  }

  const license = $('llama-license');
  if (offer && !installed) {
    license.replaceChildren(
      tr('llama.license', { model: offer.model, license: offer.license }) + ' ',
      link(offer.model_page, tr('llama.modelPage')),
    );
  } else {
    license.replaceChildren();
  }
}

function renderDockerSetup(s, p) {
  const st = s.docker_setup;
  const d = s.docker;
  const inProgress = st.state !== 'idle' && st.state !== 'failed';
  const wanted = d.state === 'missing' || (p.setupWhenNotRunning && d.state === 'not_running');
  const show = s.docker_setup_available && (inProgress || st.state === 'failed' || wanted);
  $('docker-setup').classList.toggle('hidden', !show);
  // No Docker at all: the setup button comes right under the status line, and the do-it-yourself
  // path (install one, Check again) follows as the alternative. Otherwise Check again stays first.
  const setupFirst = show && d.state === 'missing';
  const manual = $('docker-manual');
  const manualText = $('docker-manual-text');
  manual.classList.toggle('alternative', setupFirst);
  manualText.classList.toggle('hidden', !setupFirst);
  manualText.textContent = setupFirst ? tr('docker.manual', { apps: p.text('dockerApps') }) : '';
  if (setupFirst) $('docker-setup').after(manual);
  else $('docker-setup').before(manual);
  if (!show) return;
  $('docker-setup-intro').textContent = p.text('setupIntro');
  const install = $('docker-install');
  const finish = $('docker-finish');
  const text = $('docker-setup-text');
  install.classList.remove('hidden');
  install.disabled = false;
  install.textContent = d.state === 'not_running' ? tr('dockerSetup.repair') : tr('dockerSetup.install');
  finish.classList.add('hidden');
  text.className = 'muted';
  text.textContent = '';
  $('docker-setup-detail').textContent = '';
  if (st.state === 'running') {
    dot($('docker-dot'), 'busy');
    install.disabled = true;
    text.textContent = st.detail;
  } else if (st.state === 'needs_restart' || st.state === 'needs_sign_out') {
    const restart = st.state === 'needs_restart';
    dot($('docker-dot'), 'warn');
    install.classList.add('hidden');
    finish.classList.remove('hidden');
    finish.textContent = restart ? tr('dockerSetup.restartNow') : tr('dockerSetup.signOutNow');
    text.textContent = restart ? tr('dockerSetup.almostRestart') : tr('dockerSetup.almostSignOut');
  } else if (st.state === 'needs_virtualization') {
    dot($('docker-dot'), 'bad');
    install.textContent = tr('common.checkAgain');
    text.textContent = tr('dockerSetup.virtualization');
    $('docker-setup-detail').replaceChildren(link('https://learn.microsoft.com/windows/wsl/troubleshooting', tr('dockerSetup.wslGuide')));
  } else if (st.state === 'failed') {
    install.textContent = tr('common.tryAgain');
    text.className = 'error';
    text.textContent = tr('dockerSetup.failed');
    $('docker-setup-detail').textContent = st.detail;
  }
}

// Installing Tailscale from here (Windows and macOS): offered while it isn't installed, and shown
// until the user has what is left to do.
function renderTailscaleSetup(s, p) {
  const st = s.tailscale_setup;
  const done = st.state === 'installed' && s.share.state === 'on';
  const show = s.docker_setup_available && !done && (st.state !== 'idle' || !s.tailscale_installed);
  $('ts-setup').classList.toggle('hidden', !show);
  if (!show) return;
  const install = $('ts-install');
  const text = $('ts-setup-text');
  const detail = $('ts-setup-detail');
  $('ts-setup-intro').textContent = tr('tsSetup.intro', { intro: p.text('tailscaleIntro') });
  $('ts-setup-intro').classList.toggle('hidden', st.state === 'installed');
  install.classList.toggle('hidden', st.state === 'installed');
  install.disabled = st.state === 'running';
  install.textContent = st.state === 'failed' ? tr('common.tryAgain') : tr('tsSetup.install');
  text.className = st.state === 'failed' ? 'error' : 'muted';
  text.textContent = '';
  detail.textContent = '';
  if (st.state === 'running') {
    text.textContent = st.detail;
  } else if (st.state === 'installed') {
    text.textContent = tr('tsSetup.next', { next: p.text('tailscaleNext') });
    detail.replaceChildren(link('https://login.tailscale.com/admin/dns', tr('tsSetup.consoleLink')));
  } else if (st.state === 'failed') {
    text.textContent = tr('tsSetup.failed');
    detail.textContent = st.detail;
  }
}

// The language: on a first launch (none saved yet) the page asks for it before anything else;
// after that the header's Language menu changes it. It is saved in the app's config.json, so the
// tray menu and the app's own messages follow it too.
//
// Then, once, how Cedar is used: on this computer, or a server on another one
// (status.needs_choice). On a first launch the app starts nothing before that, so someone
// connecting to another computer isn't sent off to install Docker or download the sandbox image.
// Someone who set Cedar up with an earlier version (status.configured) is asked too, with their
// setup kept running meanwhile, and "this computer" worded as keeping it.
let shown = null; // 'onboarding', 'choose' or 'panel', once the first status is in

function show(which) {
  shown = which;
  $('onboarding').classList.toggle('hidden', which !== 'onboarding');
  $('choose').classList.toggle('hidden', which !== 'choose');
  $('panel').classList.toggle('hidden', which !== 'panel');
  if (which !== 'choose') chooseStep(null);
}

/** The step after the language, or the panel once the question is answered. */
function afterLanguage(s) {
  if (s && s.needs_choice) {
    renderChoose(s);
    chooseStep(null);
    show('choose');
    $('choose-local').focus();
  } else {
    show('panel');
  }
}

/** The question's wording: a first setup, or one that already runs here. */
function renderChoose(s) {
  const keep = Boolean(s.configured);
  const say = (id, key) => { $(id).dataset.i18n = key; $(id).textContent = tr(key); };
  say('choose-intro', keep ? 'choose.introExisting' : 'choose.intro');
  say('choose-local-title', keep ? 'choose.localKeep' : 'choose.local');
  say('choose-local-text', keep ? 'choose.localKeepText' : 'choose.localText');
}

// The two options, or (`remote`) the connect form, which lives in the panel's remote section and
// is moved here while the step shows, so there is one form with one set of handlers.
function chooseStep(step) {
  const remote = step === 'remote';
  $('choose-options').classList.toggle('hidden', remote);
  $('choose-remote-step').classList.toggle('hidden', !remote);
  const form = $('remote-form');
  if (remote) {
    $('choose-remote-slot').append(form);
    $('remote-server').focus();
  } else if (form.parentElement !== $('remote-offer')) {
    $('remote-offer').append(form);
  }
}

$('choose-local').addEventListener('click', async () => {
  $('choose-error').textContent = '';
  try {
    await invoke('choose_local');
    show('panel');
    await refresh();
  } catch (e) {
    $('choose-error').textContent = String(e);
  }
});
$('choose-remote').addEventListener('click', () => {
  $('choose-error').textContent = '';
  chooseStep('remote');
});
$('choose-back').addEventListener('click', () => {
  $('remote-error').textContent = '';
  chooseStep(null);
  $('choose-remote').focus();
});

/** Put the whole page into the current language. */
function relabel() {
  cedarI18n.apply(document);
  const qrShown = !$('share-qr').classList.contains('hidden');
  $('share-qr-toggle').textContent = tr(qrShown ? 'share.hideQr' : 'share.showQr');
  $('language').value = cedarI18n.locale;
  if (lastStatus) render(lastStatus);
}

function fillLanguageMenu() {
  const select = $('language');
  select.replaceChildren(...cedarI18n.LOCALES.map((l) => new Option(l.label, l.tag)));
  select.value = cedarI18n.locale;
}

function showOnboarding() {
  const suggested = cedarI18n.suggested();
  cedarI18n.use(suggested);
  const list = $('onboarding-locales');
  list.replaceChildren(...cedarI18n.LOCALES.map((l) => {
    const label = document.createElement('label');
    const input = document.createElement('input');
    input.type = 'radio';
    input.name = 'onboarding-locale';
    input.value = l.tag;
    input.checked = l.tag === suggested;
    // The button speaks the language picked, as a preview.
    input.addEventListener('change', () => {
      cedarI18n.use(l.tag);
      cedarI18n.apply($('onboarding'));
    });
    const lang = cedarI18n.LANGUAGES[l.tag.split('-')[0]];
    label.lang = l.tag;
    label.append(input, document.createTextNode(l.label));
    label.title = lang;
    return label;
  }));
  cedarI18n.apply($('onboarding'));
  show('onboarding');
  list.querySelector('input:checked')?.focus();
}

async function saveLocale(tag) {
  await invoke('set_locale', { locale: tag });
  cedarI18n.use(tag);
}

$('onboarding-continue').addEventListener('click', async () => {
  const picked = document.querySelector('input[name="onboarding-locale"]:checked');
  const tag = picked ? picked.value : cedarI18n.locale;
  $('onboarding-error').textContent = '';
  $('onboarding-continue').disabled = true;
  try {
    await saveLocale(tag);
    fillLanguageMenu();
    afterLanguage(lastStatus);
    relabel();
  } catch (e) {
    $('onboarding-error').textContent = String(e);
  } finally {
    $('onboarding-continue').disabled = false;
  }
});

$('language').addEventListener('change', async (ev) => {
  try {
    await saveLocale(ev.target.value);
    relabel();
  } catch (e) {
    ev.target.value = cedarI18n.locale;
    alert(e);
  }
});

async function refresh() {
  let s;
  try {
    s = await invoke('get_status');
  } catch (e) {
    console.error(e);
    if (!shown) show('panel');
    return;
  }
  if (!shown) {
    if (s.locale) {
      cedarI18n.use(s.locale);
      fillLanguageMenu();
      cedarI18n.apply(document);
      afterLanguage(s);
    } else {
      lastStatus = s;
      showOnboarding();
    }
  }
  // Answered elsewhere meanwhile (the menu's Restart, a config.json edited by hand).
  if (shown === 'choose' && !s.needs_choice) show('panel');
  if (shown === 'choose') renderChoose(s);
  if (shown === 'panel') render(s);
  else lastStatus = s;
}

$('docker-recheck').addEventListener('click', () => invoke('recheck').then(refresh));
$('docker-install').addEventListener('click', () => invoke('install_docker').then(refresh).catch((e) => alert(e)));
// "Set up a model for me", or after an install whose newer llama.cpp did not arrive, the launch
// sequence again (renderLlama says which).
$('llama-install').addEventListener('click', (e) => {
  invoke(e.currentTarget.dataset.command || 'install_llama').then(refresh).catch((err) => alert(err));
});
$('llama-cancel').addEventListener('click', () => invoke('cancel_llama').then(refresh).catch((e) => alert(e)));
$('llama-remove').addEventListener('click', () => {
  const model = (lastStatus && lastStatus.llama.model) || '';
  const computer = platform(lastStatus && lastStatus.platform).text('computer');
  if (!confirm(tr('llama.removeConfirm', { model, computer }))) return;
  invoke('remove_llama').then(refresh).catch((e) => alert(e));
});
$('ts-install').addEventListener('click', () => invoke('install_tailscale').then(refresh).catch((e) => alert(e)));
$('docker-finish').addEventListener('click', () => invoke('finish_docker_setup').catch((e) => alert(e)));
$('browser-toggle').addEventListener('change', (ev) => {
  invoke('set_browser', { enabled: ev.target.checked }).then(refresh).catch((e) => alert(e));
});
// The chat's Settings sends the window here with #llama for its "Run a model here with llama.cpp"
// and for "Manage…" on the model the app runs: that option is opened and scrolled to.
function openFromHash() {
  if (window.location.hash !== '#llama') return;
  sourceTouched = true;
  openSource = 'llama';
  if (lastStatus) render(lastStatus);
  $('model').scrollIntoView({ block: 'start' });
}
window.addEventListener('hashchange', openFromHash);

for (const card of document.querySelectorAll('.sources .choice')) {
  card.addEventListener('click', () => {
    sourceTouched = true;
    openSource = openSource === card.dataset.source ? null : card.dataset.source;
    if (lastStatus) render(lastStatus);
  });
}

// "List models" and "Save" of one of the two forms that name a model server: the user's own
// (`own`) or a hosted provider (`hosted`). `address` gives the address the form stands for,
// `touched` records that the user changed the form, and `filter`, where the list is long,
// narrows the model list as they type.
function modelForm({ source, address, key, probe, status, pick, select, save, error, filter, touched }) {
  let listed = [];
  const fill = () => {
    const words = filter ? $(filter).value.trim().toLowerCase().split(/\s+/).filter(Boolean) : [];
    const before = $(select).value || (lastStatus && lastStatus.llm_model);
    const shown = listed.filter((model) => words.every((word) => model.toLowerCase().includes(word)));
    $(select).replaceChildren(...shown.map((model) => new Option(model, model, false, model === before)));
    $(save).disabled = !shown.length;
  };
  if (filter) $(filter).addEventListener('input', fill);
  // A changed address or key makes the listed models someone else's.
  const changed = () => { touched(true); $(pick).classList.add('hidden'); };

  const list = async () => {
    touched(true);
    $(error).textContent = '';
    $(status).textContent = tr('model.asking');
    $(probe).disabled = true;
    try {
      listed = await invoke('list_models', { baseUrl: address(), apiKey: $(key).value });
      if (filter) $(filter).value = '';
      fill();
      $(status).textContent = tr('model.count', { count: listed.length });
      $(pick).classList.remove('hidden');
    } catch (e) {
      $(status).textContent = '';
      $(error).textContent = String(e);
    } finally {
      $(probe).disabled = false;
    }
  };
  $(probe).addEventListener('click', list);

  $(save).addEventListener('click', async () => {
    $(error).textContent = '';
    try {
      await invoke('save_model_server', { baseUrl: address(), apiKey: $(key).value, model: $(select).value, source });
      touched(false);
      // The saved option is the one in use now: the page follows it again.
      sourceTouched = false;
      $(key).value = '';
      $(pick).classList.add('hidden');
      $(status).textContent = tr('model.savedNote');
      refresh();
    } catch (e) {
      $(error).textContent = String(e);
    }
  });
  return { changed, list };
}

const ownForm = modelForm({
  source: 'own',
  address: () => $('base-url').value,
  key: 'api-key',
  probe: 'probe',
  status: 'probe-status',
  pick: 'model-pick',
  select: 'model-select',
  save: 'save',
  error: 'model-error',
  touched: (value) => { formTouched = value; },
});
// A preset fills in its server's address and asks it for its models at once: with the server
// running, one click leads to the model list.
function buildPresets() {
  if (PRESETS.length || !lastStatus) return;
  PRESETS = lastStatus.presets.map((preset) => ({ name: preset.name, url: preset.base_url, hint: preset.hint }));
  PRESETS.forEach((preset, index) => {
    const button = document.createElement('button');
    button.type = 'button';
    button.textContent = preset.name;
    button.dataset.preset = String(index);
    button.addEventListener('click', () => {
      $('base-url').value = preset.url;
      ownForm.changed();
      renderPreset();
      ownForm.list();
    });
    $('presets').appendChild(button);
  });
}
for (const id of ['base-url', 'api-key']) {
  $(id).addEventListener('input', () => {
    ownForm.changed();
    renderPreset();
  });
}

const hostedForm = modelForm({
  source: 'hosted',
  // The chosen provider's address, or the one typed for a provider that is not on the list.
  address: () => $('hosted-provider').value || $('hosted-url').value,
  key: 'hosted-key',
  probe: 'hosted-probe',
  status: 'hosted-status',
  pick: 'hosted-pick',
  select: 'hosted-select',
  save: 'hosted-save',
  error: 'hosted-error',
  filter: 'hosted-filter',
  touched: (value) => { hostedTouched = value; },
});
$('hosted-provider').addEventListener('change', () => {
  hostedForm.changed();
  // Another provider's key is not this one's.
  $('hosted-key').value = '';
  $('hosted-key').placeholder = '';
  if (lastStatus) renderHosted(lastStatus);
});
for (const id of ['hosted-url', 'hosted-key']) $(id).addEventListener('input', hostedForm.changed);

$('share-toggle').addEventListener('change', async (ev) => {
  await setShare(ev.target.checked);
});
$('share-retry').addEventListener('click', () => setShare(true));

async function setShare(enabled) {
  $('share-error').textContent = '';
  shareBusy = true;
  try {
    await invoke('set_share', { enabled });
  } catch (e) {
    // The checkbox follows the saved setting again on the refresh below.
    $('share-error').textContent = String(e);
  } finally {
    shareBusy = false;
    refresh();
  }
}
function hideQr() {
  $('share-qr').classList.add('hidden');
  $('share-qr-code').innerHTML = '';
  $('share-qr-toggle').textContent = tr('share.showQr');
}
$('share-qr-toggle').addEventListener('click', async () => {
  if (!$('share-qr').classList.contains('hidden')) {
    hideQr();
    return;
  }
  try {
    // SVG markup the app drew itself from the address and token.
    $('share-qr-code').innerHTML = await invoke('share_qr');
    $('share-qr').classList.remove('hidden');
    $('share-qr-toggle').textContent = tr('share.hideQr');
  } catch (e) {
    $('share-error').textContent = String(e);
  }
});
$('copy-share-url').addEventListener('click', () => invoke('copy_share_url').catch((e) => alert(e)));
$('share-copy-token').addEventListener('click', () => invoke('copy_token').catch((e) => alert(e)));

$('open-chat').addEventListener('click', () => invoke('open_chat').catch((e) => alert(e)));
$('open-browser').addEventListener('click', () => invoke('open_in_browser').catch((e) => alert(e)));
$('copy-token').addEventListener('click', () => invoke('copy_token').catch((e) => alert(e)));
// Links to other sites (Get Docker Desktop, Docker's terms, Tailscale) open in the default browser: the web view
// doesn't open target="_blank" links on its own.
document.addEventListener('click', (ev) => {
  const a = ev.target.closest('a[target="_blank"]');
  if (!a || !/^https?:\/\//i.test(a.href)) return;
  ev.preventDefault();
  invoke('open_url', { url: a.href }).catch((e) => alert(e));
});
$('update-install').addEventListener('click', () => invoke('install_update').then(refresh).catch((e) => alert(e)));
$('update-dismiss').addEventListener('click', () => invoke('dismiss_update').then(refresh).catch((e) => alert(e)));
$('updates-check').addEventListener('click', () => {
  // The result shows on the next refresh; a failure is in the section's text.
  invoke('check_for_updates').catch(() => {}).finally(refresh);
  setTimeout(refresh, 100);
});
$('updates-toggle').addEventListener('change', (ev) => {
  invoke('set_check_updates', { enabled: ev.target.checked }).then(refresh).catch((e) => alert(e));
});
$('open-logs').addEventListener('click', () => invoke('open_logs').catch((e) => alert(e)));

// Connecting checks the server first; once it answers, the app restarts into it and this page
// goes away, so the call only comes back when something is wrong.
$('remote-connect').addEventListener('click', async () => {
  $('remote-error').textContent = '';
  $('remote-connect').disabled = true;
  $('remote-connect-status').textContent = tr('remote.connecting');
  try {
    await invoke('connect_remote', {
      link: $('remote-link').value,
      server: $('remote-server').value,
      token: $('remote-token').value,
    });
  } catch (e) {
    $('remote-error').textContent = String(e);
  } finally {
    $('remote-connect').disabled = false;
    $('remote-connect-status').textContent = '';
  }
});
$('remote-open').addEventListener('click', () => invoke('open_chat').catch((e) => alert(e)));
$('remote-browser').addEventListener('click', () => invoke('open_in_browser').catch((e) => alert(e)));
$('remote-copy-token').addEventListener('click', () => invoke('copy_token').catch((e) => alert(e)));
$('remote-retry').addEventListener('click', () => invoke('recheck').then(refresh));
$('remote-disconnect').addEventListener('click', () => {
  $('remote-error').textContent = '';
  $('remote-disconnect').disabled = true;
  invoke('disconnect_remote')
    .catch((e) => { $('remote-error').textContent = String(e); })
    .finally(() => { $('remote-disconnect').disabled = false; });
});

refresh().then(openFromHash);
setInterval(refresh, 1500);

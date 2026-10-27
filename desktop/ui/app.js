// The setup page: polls the app for its status and sends the model server choice back.
// `window.__TAURI__` is there because tauri.conf.json sets app.withGlobalTauri.
'use strict';

const invoke = (cmd, args) => window.__TAURI__.core.invoke(cmd, args || {});
const $ = (id) => document.getElementById(id);

let lastStatus = null;
let formTouched = false;
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
    $('docker-text').textContent = tr('docker.missing', { apps: p.text('dockerApps') });
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
  if (i.state === 'waiting') {
    dot($('images-dot'), '');
    $('images-text').textContent = tr('images.waiting');
  } else if (i.state === 'pulling') {
    dot($('images-dot'), 'busy');
    $('images-text').textContent = tr('images.pulling');
    $('images-detail').textContent = i.detail;
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

  // Model server form: filled from the saved config until the user starts typing.
  if (!formTouched) {
    $('base-url').value = s.llm_base_url || '';
    if (s.has_api_key && !$('api-key').value) $('api-key').placeholder = tr('model.saved');
  }
  const m = s.model;
  $('model-detail').textContent = '';
  if (m.state === 'registering') {
    dot($('model-dot'), 'busy');
    $('model-text').textContent = tr('model.registering', { model: s.llm_model });
  } else if (m.state === 'registered') {
    dot($('model-dot'), 'ok');
    // m.detail says the same in English; the page words it in the user's language.
    $('model-text').textContent = tr('model.using', { model: s.llm_model.trim(), url: s.llm_base_url.trim() });
  } else if (m.state === 'failed') {
    dot($('model-dot'), 'bad');
    $('model-text').textContent = tr('model.failed', { model: s.llm_model });
    $('model-detail').textContent = m.detail;
  } else if (s.configured) {
    dot($('model-dot'), 'warn');
    $('model-text').textContent = tr('model.pending', { model: s.llm_model, url: s.llm_base_url });
  } else {
    dot($('model-dot'), 'warn');
    $('model-text').textContent = tr('model.notChosen');
  }

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

  renderUpdate(s);
}

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
  else key = 'updates.idle';
  $('updates-text').textContent = tr(key, params);
}

// Setting Docker up from here (Windows and macOS): offered while Docker is missing (on Windows,
// also while it won't start), and shown until it has finished.
function renderDockerSetup(s, p) {
  const st = s.docker_setup;
  const d = s.docker;
  const inProgress = st.state !== 'idle' && st.state !== 'failed';
  const wanted = d.state === 'missing' || (p.setupWhenNotRunning && d.state === 'not_running');
  const show = s.docker_setup_available && (inProgress || st.state === 'failed' || wanted);
  $('docker-setup').classList.toggle('hidden', !show);
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
let shown = null; // 'onboarding' or 'panel', once the first status is in

function show(which) {
  shown = which;
  $('onboarding').classList.toggle('hidden', which !== 'onboarding');
  $('panel').classList.toggle('hidden', which !== 'panel');
}

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
    show('panel');
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
      show('panel');
    } else {
      lastStatus = s;
      showOnboarding();
    }
  }
  if (shown === 'panel') render(s);
  else lastStatus = s;
}

$('docker-recheck').addEventListener('click', () => invoke('recheck').then(refresh));
$('docker-install').addEventListener('click', () => invoke('install_docker').then(refresh).catch((e) => alert(e)));
$('ts-install').addEventListener('click', () => invoke('install_tailscale').then(refresh).catch((e) => alert(e)));
$('docker-finish').addEventListener('click', () => invoke('finish_docker_setup').catch((e) => alert(e)));
$('browser-toggle').addEventListener('change', (ev) => {
  invoke('set_browser', { enabled: ev.target.checked }).then(refresh).catch((e) => alert(e));
});
for (const b of document.querySelectorAll('.presets button')) {
  b.addEventListener('click', () => {
    formTouched = true;
    $('base-url').value = b.dataset.url;
    $('model-pick').classList.add('hidden');
  });
}
for (const id of ['base-url', 'api-key']) {
  $(id).addEventListener('input', () => { formTouched = true; $('model-pick').classList.add('hidden'); });
}

$('probe').addEventListener('click', async () => {
  formTouched = true;
  $('model-error').textContent = '';
  $('probe-status').textContent = tr('model.asking');
  $('probe').disabled = true;
  try {
    const models = await invoke('list_models', { baseUrl: $('base-url').value, apiKey: $('api-key').value });
    const select = $('model-select');
    select.innerHTML = '';
    for (const m of models) {
      const opt = document.createElement('option');
      opt.value = m;
      opt.textContent = m;
      if (lastStatus && m === lastStatus.llm_model) opt.selected = true;
      select.appendChild(opt);
    }
    $('probe-status').textContent = tr('model.count', { count: models.length });
    $('model-pick').classList.remove('hidden');
  } catch (e) {
    $('probe-status').textContent = '';
    $('model-error').textContent = String(e);
  } finally {
    $('probe').disabled = false;
  }
});

$('save').addEventListener('click', async () => {
  $('model-error').textContent = '';
  try {
    await invoke('save_model_server', {
      baseUrl: $('base-url').value,
      apiKey: $('api-key').value,
      model: $('model-select').value,
    });
    formTouched = false;
    $('model-pick').classList.add('hidden');
    $('probe-status').textContent = tr('model.savedNote');
    refresh();
  } catch (e) {
    $('model-error').textContent = String(e);
  }
});

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

refresh();
setInterval(refresh, 1500);

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

// The page's wording for the platform the app runs on.
const PLATFORM = {
  macos: {
    computer: 'this Mac',
    dockerApps: 'Docker Desktop, OrbStack, or Colima',
    dockerStart: 'Start Docker Desktop (or OrbStack, Colima)',
  },
  windows: {
    computer: 'this PC',
    dockerApps: 'Docker Desktop (with the WSL 2 backend) or Rancher Desktop',
    dockerStart: 'Start Docker Desktop (or Rancher Desktop)',
  },
};

function render(s) {
  lastStatus = s;
  const p = PLATFORM[s.platform] || PLATFORM.macos;
  for (const e of document.querySelectorAll('.this-computer')) e.textContent = p.computer;
  $('version').textContent = 'v' + s.version;
  $('config-path').textContent = s.config_path;
  $('sandbox-image').textContent = s.sandbox_image;

  // Docker
  const d = s.docker;
  const dockerHelp = $('docker-help');
  dockerHelp.classList.add('hidden');
  if (d.state === 'checking') {
    dot($('docker-dot'), 'busy');
    $('docker-text').textContent = 'Looking for Docker…';
  } else if (d.state === 'missing') {
    dot($('docker-dot'), 'bad');
    $('docker-text').textContent = 'No docker command was found. Install ' + p.dockerApps + ', start it, then check again.';
    dockerHelp.href = 'https://www.docker.com/products/docker-desktop/';
    dockerHelp.classList.remove('hidden');
  } else if (d.state === 'starting') {
    dot($('docker-dot'), 'busy');
    $('docker-text').textContent = 'Starting Docker Desktop… The first start after installing it takes a minute or two.';
  } else if (d.state === 'not_running') {
    dot($('docker-dot'), 'warn');
    $('docker-text').textContent = 'Docker is installed but not answering. ' + p.dockerStart + ' and check again. ' + d.detail;
  } else {
    dot($('docker-dot'), 'ok');
    $('docker-text').textContent = d.detail;
  }

  renderDockerSetup(s);

  // Images
  const i = s.images;
  $('images-detail').textContent = '';
  if (i.state === 'waiting') {
    dot($('images-dot'), '');
    $('images-text').textContent = 'Waiting for Docker.';
  } else if (i.state === 'pulling') {
    dot($('images-dot'), 'busy');
    $('images-text').textContent = 'Downloading the image the agent’s tools run in. This happens once per version.';
    $('images-detail').textContent = i.detail;
  } else if (i.state === 'failed') {
    dot($('images-dot'), 'bad');
    $('images-text').textContent = 'The download failed.';
    $('images-detail').textContent = i.detail;
  } else {
    dot($('images-dot'), 'ok');
    $('images-text').textContent = 'Ready.';
  }
  const toggle = $('browser-toggle');
  if (document.activeElement !== toggle) toggle.checked = s.browser;

  // Model server form: filled from the saved config until the user starts typing.
  if (!formTouched) {
    $('base-url').value = s.llm_base_url || '';
    if (s.has_api_key && !$('api-key').value) $('api-key').placeholder = 'saved';
  }
  const m = s.model;
  $('model-detail').textContent = '';
  if (m.state === 'registering') {
    dot($('model-dot'), 'busy');
    $('model-text').textContent = 'Registering ' + s.llm_model + ' with Cedar…';
  } else if (m.state === 'registered') {
    dot($('model-dot'), 'ok');
    $('model-text').textContent = 'Using ' + m.detail;
  } else if (m.state === 'failed') {
    dot($('model-dot'), 'bad');
    $('model-text').textContent = 'Cedar could not register ' + s.llm_model + '. Is the model server running?';
    $('model-detail').textContent = m.detail;
  } else if (s.configured) {
    dot($('model-dot'), 'warn');
    $('model-text').textContent = s.llm_model + ' at ' + s.llm_base_url + ' (registered once Cedar starts)';
  } else {
    dot($('model-dot'), 'warn');
    $('model-text').textContent = 'Not chosen yet.';
  }

  // Services
  const sv = s.services;
  $('services-detail').textContent = '';
  const running = sv.state === 'running';
  if (sv.state === 'starting') {
    dot($('services-dot'), 'busy');
    $('services-text').textContent = 'Starting: ' + sv.detail;
  } else if (sv.state === 'crashed') {
    dot($('services-dot'), 'bad');
    $('services-text').textContent = 'Stopped with an error.';
    $('services-detail').textContent = sv.detail;
  } else if (running) {
    dot($('services-dot'), 'ok');
    $('services-text').textContent = 'Running at ' + s.chat_url + '.'
  } else {
    dot($('services-dot'), '');
    $('services-text').textContent = s.configured
      ? 'Not running.'
      : 'Waiting for a model server to be chosen above.';
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
    $('share-text').textContent = 'Setting up tailscale serve…';
  } else if (sh.state === 'on') {
    dot($('share-dot'), 'ok');
    $('share-text').textContent = 'In the Cedar app’s Settings (iOS or Android), scan the QR code, or enter ' + sh.detail + ' and the access token. It works while ' + p.computer + ' is awake and Cedar is running.';
  } else if (sh.state === 'failed') {
    dot($('share-dot'), 'bad');
    $('share-text').textContent = 'Not shared yet. Fix the cause, then try again.';
    $('share-detail').textContent = sh.detail;
  } else if (s.tailscale_share) {
    dot($('share-dot'), 'warn');
    $('share-text').textContent = 'Turns on once Cedar is running.';
  } else {
    dot($('share-dot'), '');
    $('share-text').textContent = 'Off. Only ' + p.computer + ' can reach the chat.';
  }

  $('open-chat').disabled = !running;
  $('open-browser').disabled = !running;
}

// Setting Docker up from here (Windows only): offered while Docker is missing or won't start, and
// shown until it has finished.
function renderDockerSetup(s) {
  const st = s.docker_setup;
  const d = s.docker;
  const inProgress = st.state !== 'idle' && st.state !== 'failed';
  const show = s.docker_setup_available && (inProgress || st.state === 'failed' || d.state === 'missing' || d.state === 'not_running');
  $('docker-setup').classList.toggle('hidden', !show);
  if (!show) return;
  const install = $('docker-install');
  const finish = $('docker-finish');
  const text = $('docker-setup-text');
  install.classList.remove('hidden');
  install.disabled = false;
  install.textContent = d.state === 'not_running' ? 'Repair WSL and Docker' : 'Set up Docker for me';
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
    finish.textContent = restart ? 'Restart now' : 'Sign out now';
    text.textContent = 'Almost done. ' + (restart ? 'Restart the PC' : 'Sign out of Windows and sign in again')
      + ' to finish; save your work in other apps first. Cedar opens again after you sign in, starts Docker Desktop, and carries on.';
  } else if (st.state === 'needs_virtualization') {
    dot($('docker-dot'), 'bad');
    install.textContent = 'Check again';
    text.textContent = 'Virtualization is turned off in this PC\u2019s firmware (BIOS or UEFI), and WSL 2 needs it. Restart the PC into its firmware settings (often by pressing F2, F10, or Del while it starts, or from Settings → System → Recovery → Advanced startup), turn on Intel VT-x, AMD-V or SVM (sometimes just “Virtualization Technology”), save, and start Windows again. Then click Check again.';
    $('docker-setup-detail').innerHTML = '<a href="https://learn.microsoft.com/windows/wsl/troubleshooting" target="_blank" rel="noreferrer">Microsoft\u2019s WSL troubleshooting guide</a>';
  } else if (st.state === 'failed') {
    install.textContent = 'Try again';
    text.className = 'error';
    text.textContent = 'Setting up Docker did not finish.';
    $('docker-setup-detail').textContent = st.detail;
  }
}

async function refresh() {
  try {
    render(await invoke('get_status'));
  } catch (e) {
    console.error(e);
  }
}

$('docker-recheck').addEventListener('click', () => invoke('recheck').then(refresh));
$('docker-install').addEventListener('click', () => invoke('install_docker').then(refresh).catch((e) => alert(e)));
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
  $('probe-status').textContent = 'Asking the server…';
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
    $('probe-status').textContent = models.length + (models.length === 1 ? ' model' : ' models');
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
    $('probe-status').textContent = 'Saved.';
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
  $('share-qr-toggle').textContent = 'Show QR code';
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
    $('share-qr-toggle').textContent = 'Hide QR code';
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
$('open-logs').addEventListener('click', () => invoke('open_logs').catch((e) => alert(e)));

refresh();
setInterval(refresh, 1500);

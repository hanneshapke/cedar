'use strict';

// The Phone page of Settings (framed by settings-shell.js, id `iphone`), shared by every UI: what
// the Cedar iOS and Android apps need to connect to this chat (GET /api/pairing). Both scan the
// same cedar://connect code and connect over HTTPS only, so the page shows the address the chat is served at (CHAT_PUBLIC_URL when it is https, normally the
// tailnet address `tailscale serve` gives it), the QR code the app scans, the access token, and
// whether Tailscale on this computer is ready, with the steps on each kind of phone. The Cedar desktop app
// (macOS, Windows) serves the chat on the tailnet from its setup page and scripts/cedar-stack.sh
// does it on Linux; this page only says how, it changes nothing.
//
// The app hands over its helpers through cedarPairing.init():
//
//   api(method, path, body) -> Promise<json>   the app's /api client (sends the access token)
//   toast(message)                             a short notice

const cedarPairing = (() => {
  const ADMIN_KEY = 'cedar-chat-admin';
  const COMPUTER = { macos: 'this Mac', windows: 'this PC' };

  let deps = null;
  let info = null;  // /api/pairing, as last loaded

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

  function note(text, className = 'cedar-set-note') {
    return el('p', className, text);
  }

  function badge(text, kind = '') {
    return el('span', `cedar-set-badge ${kind}`, text);
  }

  function code(text) {
    return el('code', '', text);
  }

  /** A paragraph from strings and nodes, so commands can be <code>. */
  function para(className, ...parts) {
    const p = el('p', className);
    p.append(...parts);
    return p;
  }

  function row(title, description) {
    const r = el('div', 'cedar-set-integration');
    const main = el('div', 'cedar-set-integration-main');
    main.append(el('strong', '', title));
    if (description) main.append(el('small', '', description));
    const side = el('div', 'cedar-set-integration-side');
    r.append(main, side);
    return { row: r, main, side };
  }

  function block(title, ...children) {
    const b = el('div', 'cedar-set-block');
    b.append(el('div', 'settings-eyebrow', title), ...children);
    return b;
  }

  function shell() {
    return typeof cedarSettingsShell === 'undefined' ? null : cedarSettingsShell;
  }

  function inDesktopApp() {
    return Boolean(window.__CEDAR_DESKTOP_APP__);
  }

  function adminToken() {
    try { return sessionStorage.getItem(ADMIN_KEY) || ''; } catch { return ''; }
  }

  function masked(token) {
    return token.length > 4 ? `••••${token.slice(-4)}` : '••••';
  }

  async function copy(text, what) {
    try {
      await navigator.clipboard.writeText(text);
      deps.toast(`${what} copied`);
    } catch {
      deps.toast('Could not copy to the clipboard');
    }
  }

  // -- the checks --------------------------------------------------------------------------------

  /** Tailscale's state as a badge. */
  function tailscaleBadge(ts) {
    switch (ts.state) {
      case 'ready': return badge('Ready', 'ok');
      case 'missing': return badge('Not installed', 'warn');
      case 'logged_out': return badge('Logged out', 'warn');
      case 'stopped': return badge('Turned off', 'warn');
      case 'connecting': return badge('Connecting', 'warn');
      case 'no_magic_dns': return badge('No MagicDNS', 'warn');
      case 'no_https': return badge('HTTPS off', 'warn');
      default: return badge('Not answering', 'warn');
    }
  }

  /**
   * What this computer needs, in order, each done or not. A step after the first one not done is
   * unknown rather than missing (`tailscale status` only says the first problem).
   */
  function computerSteps(p, computer) {
    const order = ['missing', 'unavailable', 'logged_out', 'stopped', 'connecting', 'no_magic_dns', 'no_https', 'ready'];
    const at = Math.max(0, order.indexOf(p.tailscale.state));
    const past = (state) => at > order.indexOf(state);
    const serveHow = inDesktopApp()
      ? ['Turn on ', el('b', '', 'Share with the phone apps over Tailscale'), ' on Cedar\'s setup page (', el('b', '', 'Phone Access…'), ' in the menu).']
      : ['Start the chat with ', code('scripts/cedar-stack.sh'), ' (', code('TAILSCALE_SERVE=auto'), ' or ', code('on'), ' in ', code('.env'), '), or run ', code('tailscale serve'), ' yourself and set ', code('CHAT_PUBLIC_URL'), ' to its address.'];
    return [
      { done: past('unavailable'), parts: [`Tailscale installed on ${computer}`, ' (', link('https://tailscale.com/download', 'tailscale.com/download'), ').'] },
      { done: past('connecting'), parts: ['Logged in and connected.'] },
      { done: past('no_magic_dns'), parts: ['MagicDNS turned on, so the computer has a name on the tailnet.'] },
      { done: past('no_https'), parts: ['HTTPS Certificates turned on in the ', link('https://login.tailscale.com/admin/dns', 'admin console (DNS page)'), '.'] },
      { done: Boolean(p.server), parts: p.server ? ['The chat served over HTTPS.'] : ['The chat served over HTTPS. ', ...serveHow] },
    ];
  }

  function link(href, text) {
    const a = el('a', '', text);
    a.href = href;
    a.target = '_blank';
    a.rel = 'noreferrer';
    return a;
  }

  function checklist(steps) {
    const list = el('ul', 'cedar-pair-checks');
    for (const s of steps) {
      const li = el('li', s.done ? 'done' : '');
      li.append(el('span', 'cedar-pair-mark', s.done ? '✓' : '○'), el('span', '', ''));
      li.lastChild.append(...s.parts);
      list.append(li);
    }
    return list;
  }

  // -- the page ----------------------------------------------------------------------------------

  function qrBlock(p) {
    const wrap = el('div', 'cedar-pair-qr');
    wrap.hidden = true;
    const box = el('div', 'cedar-pair-qr-code');
    wrap.append(box, note('In the Cedar app\'s Settings, tap Scan QR Code and point the camera here. The code holds the access token, so hide it again when you are done.'));
    const toggle = button('Show QR code', () => {
      if (!wrap.hidden) {
        wrap.hidden = true;
        box.replaceChildren();
        toggle.textContent = 'Show QR code';
        return;
      }
      // SVG markup the server drew itself from the address and token.
      box.innerHTML = p.qr_svg || '';
      wrap.hidden = false;
      toggle.textContent = 'Hide QR code';
    }, 'primary');
    return { toggle, wrap };
  }

  function tokenRow(title, description, token, what) {
    const { row: r, main, side } = row(title, description);
    const value = code(masked(token));
    value.className = 'cedar-pair-token';
    main.append(value);
    let shown = false;
    const reveal = button('Show', () => {
      shown = !shown;
      value.textContent = shown ? token : masked(token);
      reveal.textContent = shown ? 'Hide' : 'Show';
    });
    side.append(reveal, button('Copy', () => copy(token, what)));
    return r;
  }

  function render(section, p) {
    const computer = COMPUTER[p.platform] || 'this computer';
    const parts = [];

    // Pair: the address, the code and the tokens.
    const pair = [];
    const addr = row('Address', p.server
      ? (p.on_tailnet ? `Served on your tailnet. It works while ${computer} is awake and Cedar is running.` : 'This chat\'s HTTPS address (CHAT_PUBLIC_URL).')
      : 'The phone apps connect over HTTPS only, and the chat isn\'t served over HTTPS yet. See the steps below.');
    if (p.server) {
      addr.main.append(code(p.server));
      addr.side.append(badge('HTTPS', 'ok'), button('Copy', () => copy(p.server, 'Address')));
    } else {
      addr.side.append(badge('Not shared', 'warn'));
    }
    pair.push(addr.row);
    if (p.access_token) {
      pair.push(tokenRow('Access token', 'CHAT_ACCESS_TOKEN: the app asks for it with the address, or reads it from the QR code.', p.access_token, 'Access token'));
    } else {
      pair.push(note('No access token is set (CHAT_ACCESS_TOKEN), so anyone who can reach the address can use the chat. Set one before sharing it.', 'cedar-set-error'));
    }
    const admin = adminToken();
    if (admin) {
      pair.push(tokenRow('Admin token', 'CHAT_ADMIN_TOKEN, entered in this browser: it unlocks the Server page. The phone apps don\'t need it.', admin, 'Admin token'));
    }
    if (p.server && p.qr_svg) {
      const { toggle, wrap } = qrBlock(p);
      const acts = el('div', 'cedar-pair-acts');
      acts.append(toggle);
      pair.push(acts, wrap);
    }
    parts.push(block('Pair', ...pair));

    // Tailscale on this computer.
    const ts = p.tailscale;
    const known = ts.state === 'ready' && ts.name;
    const tsRow = row('Tailscale', known ? `On your tailnet as ${ts.name}.` : `${ts.detail[0].toUpperCase()}${ts.detail.slice(1)}.`);
    tsRow.side.append(tailscaleBadge(ts), button('Check again', () => renderPage(section)));
    parts.push(block(`On ${computer}`, tsRow.row, checklist(computerSteps(p, computer))));

    // The phones: the same code for both apps.
    const steps = () => {
      const list = el('ol', 'cedar-pair-steps');
      return { list, step: (...nodes) => { const li = el('li'); li.append(...nodes); list.append(li); } };
    };
    const ios = steps();
    ios.step(el('b', '', 'Tailscale'), ' from the App Store: log in with the same account as ', computer, ' and turn it on, so the phone is on the same tailnet.');
    ios.step(el('b', '', 'Cedar'), ' for iOS (iOS 17 or newer): from TestFlight, or built from ', code('iOS/'), ' with Xcode.');
    ios.step('In Cedar\'s Settings, tap ', el('b', '', 'Scan QR Code'), ' and scan the code above, or enter the address and the access token.');
    ios.step('Optional: allow notifications in Cedar, then choose ', el('b', '', 'iPhone push'), ' on the Notifications page to hear when a response finishes.');
    parts.push(block('On the iPhone or iPad', ios.list));
    const android = steps();
    android.step(el('b', '', 'Tailscale'), ' from Google Play or F-Droid: log in with the same account as ', computer, ' and turn it on.');
    android.step(el('b', '', 'Cedar'), ' for Android: install the APK from ', link('https://github.com/hanneshapke/cedar/releases', 'GitHub Releases'), '.');
    android.step('In Cedar\'s Settings, tap ', el('b', '', 'Scan QR code'), ' and scan the code above, or enter the address and the access token.');
    android.step('Optional: allow notifications in Cedar, then choose ', el('b', '', 'Android push'), ' on the Notifications page. A phone without Google Play services gets pushes through the ', link('https://ntfy.sh', 'ntfy'), ' app (UnifiedPush) instead.');
    parts.push(block('On an Android phone', android.list));

    section.replaceChildren(...parts);
  }

  async function renderPage(section) {
    section.replaceChildren(note('Checking…'));
    try {
      info = await deps.api('GET', '/pairing');
    } catch (e) {
      info = null;
      section.replaceChildren(note(`Could not load the pairing details: ${e.message}`, 'cedar-set-error'));
      return;
    }
    render(section, info);
    shell()?.refresh('iphone');
  }

  function summary() {
    if (!info) return null;
    if (info.server) return info.on_tailnet ? 'Shared on the tailnet' : 'Shared over HTTPS';
    return { text: 'Not shared', kind: '' };
  }

  function bind() {
    shell()?.register({
      id: 'iphone', label: 'Phone', title: 'iPhone, iPad & Android',
      description: 'Connect the Cedar iOS or Android app to this chat: the QR code, the access token, and what Tailscale needs.',
      render: renderPage, summary, footer: false,
    });
  }

  function init(helpers) {
    deps = helpers;
    if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', bind);
    else bind();
  }

  return { init };
})();

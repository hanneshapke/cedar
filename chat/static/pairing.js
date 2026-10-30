'use strict';

// The Phone page of Settings (framed by settings-shell.js, id `iphone`), shared by every UI: what
// the Cedar iOS and Android apps need to connect to this chat (GET /api/pairing). Both scan the
// same cedar://connect code and connect over HTTPS only, so the page shows the address the chat is served at (CHAT_PUBLIC_URL when it is https, normally the
// tailnet address `tailscale serve` gives it), the QR code the app scans, the access token, and
// whether Tailscale on this computer is ready, with the steps on each kind of phone. The Cedar desktop app
// (macOS, Windows) serves the chat on the tailnet from its setup page and scripts/cedar-stack.sh
// does it on Linux. Inside the desktop app, once Tailscale is ready, the page offers to turn
// sharing on: the button sends the window to /__cedar-app/share/on, which the app catches
// instead of loading (see desktop/src/window.rs), the same as ticking the switch on its setup
// page. That restarts the chat once. Anywhere else the page only says how, it changes nothing.
//
// The app hands over its helpers through cedarPairing.init():
//
//   api(method, path, body) -> Promise<json>   the app's /api client (sends the access token)
//   toast(message)                             a short notice

const cedarPairing = (() => {
  const ADMIN_KEY = 'cedar-chat-admin';
  const COMPUTER = { macos: 'pairing.computer.mac', windows: 'pairing.computer.pc' };
  // Where the desktop app's window catches the request to turn sharing on (never loaded).
  const SHARE_ON_PATH = '/__cedar-app/share/on';

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

  /** The text for `key` as strings and nodes: each {name} that `nodes` has becomes that node,
   *  so commands can be <code> and names <b> inside a translated sentence. */
  function rich(key, nodes, params) {
    const out = [];
    for (const [i, piece] of tr(key, params).split(/\{(\w+)\}/).entries()) {
      if (i % 2 === 0) { if (piece) out.push(piece); } else out.push(nodes[piece] ?? `{${piece}}`);
    }
    return out;
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

  /** The desktop app can turn sharing on from here, once Tailscale is ready for it. */
  function canTurnOn(p) {
    return !p.server && inDesktopApp() && p.tailscale.state === 'ready';
  }

  /** Asks the desktop app to share the chat. It restarts the chat, so the page just waits. */
  function turnOnButton() {
    const b = button(tr('pairing.turnOn'), () => {
      b.disabled = true;
      b.textContent = tr('pairing.turningOn');
      window.location.href = SHARE_ON_PATH;
    }, 'primary');
    return b;
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
      deps.toast(tr('pairing.copied', { what }));
    } catch {
      deps.toast(tr('pairing.copyFailed'));
    }
  }

  // -- the checks --------------------------------------------------------------------------------

  /** Tailscale's state as a badge. */
  function tailscaleBadge(ts) {
    switch (ts.state) {
      case 'ready': return badge(tr('pairing.ts.ready'), 'ok');
      case 'missing': return badge(tr('pairing.ts.missing'), 'warn');
      case 'logged_out': return badge(tr('pairing.ts.loggedOut'), 'warn');
      case 'stopped': return badge(tr('pairing.ts.stopped'), 'warn');
      case 'connecting': return badge(tr('pairing.ts.connecting'), 'warn');
      case 'no_magic_dns': return badge(tr('pairing.ts.noMagicDns'), 'warn');
      case 'no_https': return badge(tr('pairing.ts.noHttps'), 'warn');
      default: return badge(tr('pairing.ts.unavailable'), 'warn');
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
    const serveHow = canTurnOn(p)
      ? rich('pairing.serveButton', { button: el('b', '', tr('pairing.turnOn')) })
      : inDesktopApp()
      ? rich('pairing.serveDesktop', { share: el('b', '', tr('pairing.shareLabel')), menu: el('b', '', tr('pairing.phoneAccess')) })
      : rich('pairing.serveLinux', {
        script: code('scripts/cedar-stack.sh'), auto: code('TAILSCALE_SERVE=auto'), on: code('on'), env: code('.env'),
        serve: code('tailscale serve'), url: code('CHAT_PUBLIC_URL'),
      });
    return [
      { done: past('unavailable'), parts: rich('pairing.check.installed', { link: link('https://tailscale.com/download', 'tailscale.com/download') }, { computer }) },
      { done: past('connecting'), parts: [tr('pairing.check.connected')] },
      { done: past('no_magic_dns'), parts: [tr('pairing.check.magicDns')] },
      { done: past('no_https'), parts: rich('pairing.check.https', { link: link('https://login.tailscale.com/admin/dns', tr('pairing.check.adminConsole')) }) },
      { done: Boolean(p.server), parts: p.server ? [tr('pairing.check.served')] : [`${tr('pairing.check.served')} `, ...serveHow] },
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
    wrap.append(box, note(tr('pairing.qrNote')));
    const toggle = button(tr('pairing.showQr'), () => {
      if (!wrap.hidden) {
        wrap.hidden = true;
        box.replaceChildren();
        toggle.textContent = tr('pairing.showQr');
        return;
      }
      // SVG markup the server drew itself from the address and token.
      box.innerHTML = p.qr_svg || '';
      wrap.hidden = false;
      toggle.textContent = tr('pairing.hideQr');
    }, 'primary');
    return { toggle, wrap };
  }

  function tokenRow(title, description, token, what) {
    const { row: r, main, side } = row(title, description);
    const value = code(masked(token));
    value.className = 'cedar-pair-token';
    main.append(value);
    let shown = false;
    const reveal = button(tr('pairing.show'), () => {
      shown = !shown;
      value.textContent = shown ? token : masked(token);
      reveal.textContent = shown ? tr('pairing.hide') : tr('pairing.show');
    });
    side.append(reveal, button(tr('pairing.copy'), () => copy(token, what)));
    return r;
  }

  function render(section, p) {
    const computer = tr(COMPUTER[p.platform] || 'pairing.computer.other');
    const parts = [];

    // Pair: the address, the code and the tokens.
    const pair = [];
    const addr = row(tr('pairing.address'), p.server
      ? (p.on_tailnet ? tr('pairing.addressTailnet', { computer }) : tr('pairing.addressPublic'))
      : tr('pairing.addressNone'));
    if (p.server) {
      addr.main.append(code(p.server));
      addr.side.append(badge('HTTPS', 'ok'), button(tr('pairing.copy'), () => copy(p.server, tr('pairing.address'))));
    } else {
      addr.side.append(badge(tr('pairing.notShared'), 'warn'));
      if (canTurnOn(p)) addr.side.append(turnOnButton());
    }
    pair.push(addr.row);
    if (p.access_token) {
      pair.push(tokenRow(tr('pairing.accessToken'), tr('pairing.accessTokenNote'), p.access_token, tr('pairing.accessToken')));
    } else {
      pair.push(note(tr('pairing.noAccessToken'), 'cedar-set-error'));
    }
    const admin = adminToken();
    if (admin) {
      pair.push(tokenRow(tr('pairing.adminToken'), tr('pairing.adminTokenNote'), admin, tr('pairing.adminToken')));
    }
    if (p.server && p.qr_svg) {
      const { toggle, wrap } = qrBlock(p);
      const acts = el('div', 'cedar-pair-acts');
      acts.append(toggle);
      pair.push(acts, wrap);
    }
    parts.push(block(tr('pairing.pair'), ...pair));

    // Tailscale on this computer.
    const ts = p.tailscale;
    const known = ts.state === 'ready' && ts.name;
    const tsRow = row('Tailscale', known ? tr('pairing.onTailnetAs', { name: ts.name }) : `${ts.detail[0].toUpperCase()}${ts.detail.slice(1)}.`);
    tsRow.side.append(tailscaleBadge(ts), button(tr('pairing.checkAgain'), () => renderPage(section)));
    parts.push(block(tr('pairing.onComputer', { computer }), tsRow.row, checklist(computerSteps(p, computer))));

    // The phones: the same code for both apps.
    const steps = () => {
      const list = el('ol', 'cedar-pair-steps');
      return { list, step: (...nodes) => { const li = el('li'); li.append(...nodes); list.append(li); } };
    };
    const ios = steps();
    const b = (text) => el('b', '', text);
    ios.step(...rich('pairing.ios.tailscale', { app: b('Tailscale') }, { computer }));
    ios.step(...rich('pairing.ios.cedar', { app: b('Cedar'), dir: code('iOS/') }));
    ios.step(...rich('pairing.ios.scan', { button: b(tr('pairing.ios.scanButton')) }));
    ios.step(...rich('pairing.ios.push', { method: b(tr('notify.method.ios')) }));
    parts.push(block(tr('pairing.ios.title'), ios.list));
    const android = steps();
    android.step(...rich('pairing.android.tailscale', { app: b('Tailscale') }, { computer }));
    android.step(...rich('pairing.android.cedar', { app: b('Cedar'), link: link('https://github.com/hanneshapke/cedar/releases', 'GitHub Releases') }));
    android.step(...rich('pairing.android.scan', { button: b(tr('pairing.android.scanButton')) }));
    android.step(...rich('pairing.android.push', { method: b(tr('notify.method.android')), link: link('https://ntfy.sh', 'ntfy') }));
    parts.push(block(tr('pairing.android.title'), android.list));

    section.replaceChildren(...parts);
  }

  async function renderPage(section) {
    section.replaceChildren(note(tr('pairing.checking')));
    try {
      info = await deps.api('GET', '/pairing');
    } catch (e) {
      info = null;
      section.replaceChildren(note(tr('pairing.loadFailed', { error: e.message }), 'cedar-set-error'));
      return;
    }
    render(section, info);
    shell()?.refresh('iphone');
  }

  function summary() {
    if (!info) return null;
    if (info.server) return info.on_tailnet ? tr('pairing.sharedTailnet') : tr('pairing.sharedHttps');
    return { text: tr('pairing.notShared'), kind: '' };
  }

  function bind() {
    shell()?.register({
      id: 'iphone', label: tr('pairing.label'), title: tr('pairing.title'),
      description: tr('pairing.description'),
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

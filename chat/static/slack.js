'use strict';

// The Slack page of Settings (framed by settings-shell.js, id `slack`), shared by every UI: whether
// this server has a Slack app people can talk to Cedar through (CHAT_SLACK_APP_TOKEN and
// CHAT_SLACK_BOT_TOKEN, see chat/README.md#slack), and the caller's Slack accounts. An account is
// linked with the code the app sends it when it messages the app the first time (GET /api/slack,
// POST /api/slack/link, DELETE /api/slack/links/{team}/{user}).
//
// The app hands over its helpers through cedarSlack.init():
//
//   api(method, path, body) -> Promise<json>   the app's /api client (sends the access token)
//   toast(message)                             a short notice

const cedarSlack = (() => {
  let deps = null;
  let info = null;  // /api/slack, as last loaded
  let section = null;

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

  const note = (text, className = 'cedar-set-note') => el('p', className, text);
  const shell = () => (typeof cedarSettingsShell === 'undefined' ? null : cedarSettingsShell);

  function linkForm() {
    const form = el('form', 'cedar-set-inline');
    const input = el('input');
    input.type = 'text';
    input.autocomplete = 'off';
    input.spellcheck = false;
    input.placeholder = 'ABCD-EFGH';
    input.maxLength = 12;
    input.setAttribute('aria-label', tr('slack.codeLabel'));
    const linkBtn = button(tr('slack.link'), () => form.requestSubmit(), 'primary');
    form.append(input, linkBtn);
    form.addEventListener('submit', async (e) => {
      e.preventDefault();
      const code = input.value.trim();
      if (!code) return;
      linkBtn.disabled = true;
      try {
        const res = await deps.api('POST', '/slack/link', { code });
        deps.toast(tr('slack.linked', { name: res.link?.name || res.link?.slack_user || '' }));
        await renderPage(section);
      } catch (err) {
        linkBtn.disabled = false;
        shell()?.setStatus('slack', tr('slack.linkFailed', { error: err.message }), 'error');
      }
    });
    return form;
  }

  function linkRow(link) {
    const row = el('div', 'cedar-set-row');
    const main = el('div', 'cedar-set-row-main');
    main.append(el('span', 'cedar-set-row-name', link.name || link.slack_user));
    main.append(el('span', 'cedar-set-row-sub', link.team_name || link.team));
    const side = el('span', 'cedar-set-row-side');
    side.append(button(tr('slack.unlink'), async () => {
      try {
        await deps.api('DELETE', `/slack/links/${encodeURIComponent(link.team)}/${encodeURIComponent(link.slack_user)}`);
        deps.toast(tr('slack.unlinked', { name: link.name || link.slack_user }));
        await renderPage(section);
      } catch (err) {
        shell()?.setStatus('slack', err.message, 'error');
      }
    }));
    row.append(el('span', ''), main, side);
    return row;
  }

  function render(target) {
    target.textContent = '';
    if (!info.available) {
      target.append(note(tr('slack.notSetUp')));
      return;
    }
    const bot = info.workspace?.bot ? `@${info.workspace.bot}` : tr('slack.theApp');
    target.append(note(info.workspace
      ? tr('slack.connected', { team: info.workspace.team_name || info.workspace.team, bot })
      : tr('slack.connecting')));
    const steps = el('ol', 'cedar-set-steps');
    for (const key of ['slack.step1', 'slack.step2', 'slack.step3']) steps.append(el('li', '', tr(key, { bot })));
    target.append(steps);
    target.append(linkForm());
    if (info.links.length) {
      target.append(el('h3', '', tr('slack.accounts')));
      for (const link of info.links) target.append(linkRow(link));
    }
    target.append(note(tr('slack.howItWorks')));
  }

  async function renderPage(target) {
    section = target;
    target.replaceChildren(note(tr('slack.loading')));
    try {
      info = await deps.api('GET', '/slack');
    } catch (e) {
      info = null;
      target.replaceChildren(note(tr('slack.loadFailed', { error: e.message }), 'cedar-set-error'));
      return;
    }
    render(target);
    shell()?.refresh('slack');
  }

  function summary() {
    if (!info) return null;
    if (!info.available) return tr('slack.off');
    return info.links.length ? { text: tr('slack.linkedSummary'), kind: 'ok' } : tr('slack.notLinked');
  }

  function bind() {
    const s = shell();
    if (!s) return;
    // Every UI's Settings has a body to add the page to; only some carry its section already.
    const body = document.getElementById('settings-body');
    if (body && !body.querySelector('.settings-page[data-page="slack"]')) {
      const page = el('section', 'settings-page');
      page.dataset.page = 'slack';
      page.id = 'slack-section';
      page.hidden = true;
      page.setAttribute('aria-live', 'polite');
      const server = body.querySelector('.settings-page[data-page="server"]');
      body.insertBefore(page, server);
    }
    s.register({
      id: 'slack', label: tr('slack.label'), title: tr('slack.title'),
      description: tr('slack.description'),
      render: renderPage, summary, footer: false,
    });
    // The nav's line, before the page is first opened.
    deps.api('GET', '/slack').then((loaded) => { info = loaded; s.refresh('slack'); }).catch(() => {});
  }

  function init(helpers) {
    deps = helpers;
    if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', bind);
    else bind();
  }

  return { init };
})();

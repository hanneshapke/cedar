'use strict';

// Private chats, shared by every UI. A private conversation is for something you want neither
// remembered nor kept: it is in no conversation list, search or inbox, Cedar reads and writes no
// memory or records in it and keeps no trace of it, nothing about it is sent as a notification,
// and it is deleted when you leave it (the server deletes one that was never left, such as a
// closed tab, once it has been idle for a day).
//
// Three pieces:
//
//   - A "Private" button in the composer. On the new-chat screen it decides whether the chat
//     about to start is private; in a private chat it is shown pressed, and can't be changed (a
//     conversation is private from its first message or not at all).
//   - A note above the message box that says what private means, with "End chat" once the chat
//     has started.
//   - Leaving: when the open conversation changes away from a private one, that one is deleted
//     (DELETE /api/conversations/{id}), a response still being written included.
//
// While a private chat is open (or about to start) <body> has the class `cedar-private`, and
// `cedar-private-open` once it has started: private.css hides what a private chat doesn't have
// (the star, the trace, contributing, making an agent of it).
//
// Each app hands over its helpers through cedarPrivate.init(), asks cedarPrivate.wanted() when it
// creates a conversation, and calls cedarPrivate.setCurrent(conversation) whenever the open
// conversation changes (where it calls cedarStar.setCurrent):
//
//   api(method, path, body) -> Promise<json>   the app's /api client (sends the access token)
//   toast(message)                             a short notice
//   form                                       the composer <form>
//   anchor                                     the model picker; the button goes after it and the
//                                              Tools and paperclip buttons that follow it
//   before                                     the element the note goes above (the message box)

const cedarPrivate = (() => {
  const ICON = '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M3 3l18 18M10.5 6.2A9.6 9.6 0 0 1 12 6c5 0 8.5 4 9.5 6a13 13 0 0 1-2.7 3.5M6.4 7.7A13.5 13.5 0 0 0 2.5 12c1 2 4.5 6 9.5 6a9.4 9.4 0 0 0 3.8-.8M9.9 9.9a3 3 0 0 0 4.2 4.2"/></svg>';

  let deps = null;
  let wanted = false;   // the chat about to start is private
  let current = null;   // the open conversation, as the app last handed it over
  let button = null;
  let note = null;
  let text = null;
  let end = null;

  function el(tag, className, text) {
    const e = document.createElement(tag);
    if (className) e.className = className;
    if (text !== undefined) e.textContent = text;
    return e;
  }

  function render() {
    const open = Boolean(current?.private);
    const on = open || (!current && wanted);
    document.body.classList.toggle('cedar-private', on);
    document.body.classList.toggle('cedar-private-open', open);
    if (!button) return;
    // Only where it means something: before a chat starts, and in a private one.
    button.hidden = Boolean(current) && !open;
    button.disabled = open;
    button.setAttribute('aria-pressed', String(on));
    button.title = open ? tr('private.openTitle') : on ? tr('private.onTitle') : tr('private.offTitle');
    note.hidden = !on;
    // The whole of it before the chat starts; a reminder once it has.
    text.textContent = open ? tr('private.openNote') : tr('private.note');
    end.hidden = !open;
  }

  /** Delete a private conversation that was left. One the page already deleted is fine. */
  async function leave(conversation) {
    try {
      await deps.api('DELETE', `/conversations/${conversation.id}`);
      deps.toast(tr('private.ended'));
    } catch { /* already gone, or the server can't be reached: it deletes idle ones itself */ }
  }

  function bind() {
    const { form, anchor, before } = deps;
    button = el('button', 'copy-btn cedar-private-btn');
    button.type = 'button';
    button.innerHTML = ICON;
    button.append(el('span', 'lbl', tr('private.button')));
    button.addEventListener('click', () => { wanted = !wanted; render(); });
    // After the Tools button and the paperclip (with its file input), which follow the anchor.
    const clip = form.querySelector('.cedar-attach-btn');
    const picker = clip?.nextElementSibling;
    const last = picker?.matches('input[type="file"]') ? picker : clip || form.querySelector('.cedar-set-chip') || anchor;
    last.after(button);

    note = el('div', 'cedar-private-note');
    note.setAttribute('role', 'status');
    note.insertAdjacentHTML('afterbegin', ICON);
    text = el('span', 'cedar-private-text');
    note.append(text);
    end = el('button', 'copy-btn cedar-private-end', tr('private.end'));
    end.type = 'button';
    end.title = tr('private.endTitle');
    // Leaving through the app's own route: the hash change opens the new-chat screen, which
    // hands over `null`, and that is what deletes the conversation.
    end.addEventListener('click', () => { location.hash = '#/'; });
    note.append(end);
    before.before(note);
    render();
  }

  function init(helpers) {
    deps = helpers;
    if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', bind);
    else bind();
  }

  /** The open conversation changed (`null` when none is open). Apps call this on every repaint
   *  of their header, so it only acts when the conversation is another one. */
  function setCurrent(conversation) {
    const before = current;
    current = conversation || null;
    if (before?.id !== current?.id) {
      if (before?.private) leave(before);
      // The next new chat is an ordinary one again, unless this is the private chat just started.
      if (current) wanted = false;
    }
    render();
  }

  return { init, setCurrent, wanted: () => wanted && !current };
})();

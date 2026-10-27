'use strict';

// An in-page stand-in for window.confirm(), resolving to true or false. The Cedar app for macOS
// shows the chat in a WKWebView without JavaScript dialogs, where confirm() returns false without
// asking anything, so a "Delete …?" there did nothing. Every UI loads it; it draws with the UI's
// own `dialog`, `.dialog-actions`, `.primary` and `.ghost` styles.

function cedarConfirm(message, { ok = tr('confirm.ok') } = {}) {
  return new Promise((resolve) => {
    const dialog = document.createElement('dialog');
    dialog.className = 'cedar-confirm';
    const text = document.createElement('p');
    text.style.whiteSpace = 'pre-line';
    text.textContent = message;
    // A `dialog` form closes the dialog with the pressed button's value; Escape closes it with none.
    const form = document.createElement('form');
    form.method = 'dialog';
    form.className = 'dialog-actions';
    form.style.gap = '8px';
    const button = (label, cls, value) => {
      const b = document.createElement('button');
      b.className = cls;
      b.value = value;
      b.textContent = label;
      return b;
    };
    const yes = button(ok, 'primary', 'ok');
    form.append(button(tr('confirm.cancel'), 'ghost', 'cancel'), yes);
    dialog.append(text, form);
    dialog.addEventListener('close', () => {
      dialog.remove();
      resolve(dialog.returnValue === 'ok');
    });
    document.body.append(dialog);
    dialog.showModal();
    yes.focus();
  });
}

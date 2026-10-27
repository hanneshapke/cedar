'use strict';

// Attaching images and PDFs to a message, shared by both UIs (/v1 and /v2). PDFs go with any
// model (cedar saves them in the run's workspace for the agent to read); images only while the
// model the picker has chosen is registered as accepting them (see models.js; the app calls
// setImages as the picker changes, which also shows the button). Three ways in:
// a paperclip button by the model picker, dropping files on the message box, and pasting an
// image. Files are read in the browser as base64 and go with the message as
// `attachments: [{name, media_type, data}]`; the limits here mirror the server's.
//
// Each app hands over its elements through cedarAttach.init():
//
//   form      the composer form (the drop target)
//   textarea  the message box (paste target)
//   anchor    the model picker; the paperclip goes after it, or after the Tools button that
//             settings.js puts there (its init runs first, so the button exists by then)
//   before    the element the row of chosen files goes before
//   toast(message)   a short notice

const cedarAttach = (() => {
  const TYPES = ['image/png', 'image/jpeg', 'image/gif', 'image/webp', 'application/pdf'];
  const ACCEPT = TYPES.join(',');
  const MAX_FILES = 10;
  const MAX_BYTES = 20 * 1024 * 1024;
  const MAX_TOTAL_BYTES = 32 * 1024 * 1024;
  const CLIP = '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M21.4 11.1l-8.8 8.8a5.5 5.5 0 0 1-7.8-7.8l8.8-8.8a3.7 3.7 0 0 1 5.2 5.2l-8.8 8.8a1.8 1.8 0 0 1-2.6-2.6l8.1-8.1"/></svg>';

  const PDF = 'application/pdf';

  let deps = null;
  let enabled = false;  // set once the app knows the model
  let images = false;   // the chosen model takes images too
  let files = [];       // [{name, media_type, size, data, url}]
  let button = null;
  let picker = null;
  let row = null;
  let dragDepth = 0;

  function el(tag, className, text) {
    const e = document.createElement(tag);
    if (className) e.className = className;
    if (text !== undefined) e.textContent = text;
    return e;
  }

  function fmtSize(n) {
    const num = (v, digits) => cedarI18n.number(v, { minimumFractionDigits: digits, maximumFractionDigits: digits, useGrouping: false });
    if (n < 1024) return tr('attach.sizeB', { size: num(n, 0) });
    if (n < 1024 * 1024) return tr('attach.sizeKB', { size: num(n / 1024, n < 10 * 1024 ? 1 : 0) });
    return tr('attach.sizeMB', { size: num(n / (1024 * 1024), 1) });
  }

  function kindOf(file) {
    if (TYPES.includes(file.type)) return file.type;
    if (/\.pdf$/i.test(file.name)) return 'application/pdf';
    return null;
  }

  function readBase64(file) {
    return new Promise((resolve, reject) => {
      const reader = new FileReader();
      reader.onerror = () => reject(new Error(tr('attach.readFailed', { name: file.name })));
      // A data URL: "data:<type>;base64,<data>". Only the part after the comma is sent.
      reader.onload = () => resolve(String(reader.result).slice(String(reader.result).indexOf(',') + 1));
      reader.readAsDataURL(file);
    });
  }

  async function add(list) {
    if (!enabled || !list || !list.length) return;
    const total = () => files.reduce((n, f) => n + f.size, 0);
    for (const file of Array.from(list)) {
      const media_type = kindOf(file);
      if (!media_type) { deps.toast(tr('attach.wrongType', { name: file.name })); continue; }
      if (media_type !== PDF && !images) { deps.toast(tr('attach.noImages', { name: file.name })); continue; }
      if (!file.size) { deps.toast(tr('attach.empty', { name: file.name })); continue; }
      if (file.size > MAX_BYTES) { deps.toast(tr('attach.tooLarge', { name: file.name, size: fmtSize(MAX_BYTES) })); continue; }
      if (files.length >= MAX_FILES) { deps.toast(tr('attach.tooMany', { count: MAX_FILES })); break; }
      if (total() + file.size > MAX_TOTAL_BYTES) { deps.toast(tr('attach.tooMuch', { size: fmtSize(MAX_TOTAL_BYTES) })); break; }
      let data;
      try {
        data = await readBase64(file);
      } catch (e) {
        deps.toast(e.message);
        continue;
      }
      if (!enabled || (media_type !== PDF && !images)) return; // the model changed while reading
      const url = media_type.startsWith('image/') ? URL.createObjectURL(file) : null;
      files.push({ name: file.name, media_type, size: file.size, data, url });
    }
    render();
  }

  function remove(entry) {
    if (entry.url) URL.revokeObjectURL(entry.url);
    files = files.filter((f) => f !== entry);
    render();
  }

  function chip(info, onRemove) {
    const c = el('span', 'cedar-attach-chip');
    c.title = `${info.name} (${fmtSize(info.size)})`;
    if (info.url) {
      const img = el('img', 'cedar-attach-thumb');
      img.src = info.url;
      img.alt = '';
      c.append(img);
    } else {
      c.append(el('span', 'cedar-attach-kind', info.media_type === 'application/pdf' ? 'PDF' : 'IMG'));
    }
    c.append(el('span', 'cedar-attach-name', info.name), el('span', 'cedar-attach-size', fmtSize(info.size)));
    if (onRemove) {
      const x = el('button', 'cedar-attach-remove', '×');
      x.type = 'button';
      x.setAttribute('aria-label', tr('attach.remove', { name: info.name }));
      x.addEventListener('click', onRemove);
      c.append(x);
    }
    return c;
  }

  /** Static chips for a sent message, from the turn's `attachments` (no bytes, so no thumbnail). */
  function chips(infos) {
    const r = el('span', 'cedar-attach-sent');
    for (const info of infos || []) r.append(chip(info));
    return r;
  }

  function render() {
    if (!row) return;
    row.replaceChildren(...files.map((f) => chip(f, () => remove(f))));
    row.hidden = files.length === 0;
  }

  /** Offer the button; images too if `on`. Chosen images go when the model can't take them. */
  function setImages(on) {
    enabled = true;
    images = Boolean(on);
    syncButton();
    if (!images && files.some((f) => f.media_type !== PDF)) {
      for (const f of files) if (f.url) URL.revokeObjectURL(f.url);
      files = files.filter((f) => f.media_type === PDF);
      render();
    }
  }

  function clear() {
    for (const f of files) if (f.url) URL.revokeObjectURL(f.url);
    files = [];
    render();
  }

  function syncButton() {
    if (!button) return;
    const what = images ? tr('attach.imagesOrPdfs') : tr('attach.pdfs');
    button.hidden = !enabled;
    button.title = what;
    button.setAttribute('aria-label', what);
    picker.accept = images ? ACCEPT : PDF;
  }

  /** What goes with the message: `[{name, media_type, data}]`. */
  function payload() {
    return files.map(({ name, media_type, data }) => ({ name, media_type, data }));
  }

  function hasFiles(e) {
    return Array.from(e.dataTransfer?.types || []).includes('Files');
  }

  function bind() {
    const { form, textarea, anchor, before } = deps;
    picker = el('input');
    picker.type = 'file';
    picker.multiple = true;
    picker.hidden = true;
    picker.addEventListener('change', () => { add(picker.files); picker.value = ''; });

    button = el('button', 'copy-btn cedar-attach-btn');
    button.type = 'button';
    button.innerHTML = CLIP;
    syncButton();
    button.addEventListener('click', () => picker.click());
    const tools = anchor.nextElementSibling;
    (tools?.classList.contains('cedar-set-chip') ? tools : anchor).after(button, picker);

    row = el('div', 'cedar-attach-row');
    row.hidden = true;
    before.before(row);

    form.addEventListener('dragenter', (e) => {
      if (!enabled || !hasFiles(e)) return;
      e.preventDefault();
      dragDepth += 1;
      form.classList.add('cedar-drag');
    });
    form.addEventListener('dragover', (e) => {
      if (!enabled || !hasFiles(e)) return;
      e.preventDefault();
      e.dataTransfer.dropEffect = 'copy';
    });
    form.addEventListener('dragleave', () => {
      if (dragDepth > 0) dragDepth -= 1;
      if (dragDepth === 0) form.classList.remove('cedar-drag');
    });
    form.addEventListener('drop', (e) => {
      if (!enabled || !hasFiles(e)) return;
      e.preventDefault();
      dragDepth = 0;
      form.classList.remove('cedar-drag');
      add(e.dataTransfer.files);
    });
    textarea.addEventListener('paste', (e) => {
      if (!enabled) return;
      const pasted = Array.from(e.clipboardData?.items || []).filter((i) => i.kind === 'file').map((i) => i.getAsFile()).filter(Boolean);
      if (!pasted.length) return;
      e.preventDefault();
      add(pasted);
    });
  }

  function init(helpers) {
    deps = helpers;
    if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', bind);
    else bind();
  }

  return { init, setImages, enabled: () => enabled, count: () => files.length, payload, clear, chips, fmtSize };
})();

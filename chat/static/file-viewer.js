'use strict';

// Workspace file viewer, shared by both UIs (/v1 and /v2). It shows a Markdown file as rendered
// Markdown and a PDF in the browser's own PDF viewer, in a modal dialog. Each app hands over its
// own helpers through cedarViewer.init(), so this file knows nothing about the app's internals:
//
//   fetchFile(path)   -> Promise<Response>  an ok response for a workspace file (sends the access token)
//   renderMarkdown(s) -> html string        the app's Markdown renderer, which escapes all text
//   download(path, button)                  starts a download of the file
//
// The files are fetched with the Authorization header and shown from a blob: URL, because the API
// takes its token from that header only, so a plain <iframe src="/api/..."> would be rejected.

const cedarViewer = (() => {
  const MAX_MARKDOWN_BYTES = 2 * 1024 * 1024;
  const KINDS = [[/\.(md|markdown)$/i, 'markdown'], [/\.pdf$/i, 'pdf']];

  let deps = null;
  let ui = null;       // the dialog and its parts, built on first use
  let current = null;  // { path, kind } of the file being shown
  let pdfBlob = null;
  let pdfUrl = null;
  let request = 0;     // bumped on every open and close, so a slow fetch can't draw into a newer view

  /** 'markdown', 'pdf', or null when the file can't be previewed. */
  function kind(path) {
    if (typeof path !== 'string') return null;
    return KINDS.find(([re]) => re.test(path))?.[1] ?? null;
  }

  function el(tag, className, text) {
    const e = document.createElement(tag);
    if (className) e.className = className;
    if (text !== undefined) e.textContent = text;
    return e;
  }

  function button(label, onClick, icon) {
    const b = el('button', 'copy-btn');
    b.type = 'button';
    if (icon) b.innerHTML = icon;
    b.append(label);
    b.addEventListener('click', onClick);
    return b;
  }

  /** A "View" button for `path`, or null when it isn't a file the viewer can show. */
  function viewButton(path, { label = 'View', primary = false, icon } = {}) {
    if (!deps || !kind(path)) return null;
    const b = button(label, () => open(path), icon);
    if (primary) b.classList.add('primary');
    b.title = `View ${path}`;
    return b;
  }

  function build() {
    const dialog = el('dialog', 'viewer-dialog');
    dialog.setAttribute('aria-labelledby', 'viewer-title');
    const head = el('div', 'viewer-head');
    const title = el('h3', 'viewer-title');
    title.id = 'viewer-title';
    const actions = el('div', 'viewer-actions');
    const openTab = button('Open in new tab', openInNewTab);
    const download = button('Download', (e) => current && deps.download(current.path, e.currentTarget));
    const close = button('Close', () => hide());
    actions.append(openTab, download, close);
    head.append(title, actions);
    const body = el('div', 'viewer-body');
    dialog.append(head, body);
    // The browser fires `close` a moment after the dialog closes. If another file was opened in
    // between, that late event must not wipe it.
    dialog.addEventListener('close', () => { if (!dialog.open) reset(); });
    document.body.append(dialog);
    return { dialog, title, body, openTab };
  }

  function setBody(node, mode) {
    ui.body.className = `viewer-body${mode ? ` ${mode}` : ''}`;
    ui.body.replaceChildren(node);
  }

  function setMessage(text) {
    setBody(el('div', 'viewer-message', text));
  }

  /** Forget the open file: drop any pending fetch, free the PDF, empty the body. */
  function reset() {
    request++;
    if (pdfUrl) URL.revokeObjectURL(pdfUrl);
    pdfUrl = pdfBlob = current = null;
    if (ui) {
      ui.body.className = 'viewer-body';
      ui.body.replaceChildren();
      ui.openTab.hidden = true;
    }
  }

  /** Close the dialog (if open) and forget the file right away. */
  function hide() {
    if (ui?.dialog.open) ui.dialog.close();
    reset();
  }

  function openInNewTab() {
    if (!pdfBlob) return;
    const url = URL.createObjectURL(pdfBlob); // its own URL, so closing the dialog can't pull the file away
    window.open(url, '_blank', 'noopener');
    setTimeout(() => URL.revokeObjectURL(url), 60_000);
  }

  async function showMarkdown(res, id) {
    const tooLarge = () => setMessage('This file is too large to preview. Download it instead.');
    if (Number(res.headers.get('Content-Length')) > MAX_MARKDOWN_BYTES) return tooLarge();
    const text = await res.text();
    if (id !== request) return;
    if (text.length > MAX_MARKDOWN_BYTES) return tooLarge();
    if (!text.trim()) return setMessage('This file is empty.');
    const page = el('div', 'md');
    page.innerHTML = deps.renderMarkdown(text);
    setBody(page, 'markdown');
  }

  async function showPdf(res, id, name) {
    const bytes = await res.arrayBuffer();
    if (id !== request) return;
    // A PDF starts with "%PDF-" within its first KiB. Anything else is not shown, whatever its name.
    const start = new TextDecoder('latin1').decode(new Uint8Array(bytes, 0, Math.min(1024, bytes.byteLength)));
    if (!start.includes('%PDF-')) return setMessage('This file is not a valid PDF. You can still download it.');
    // The type is forced, so the browser's PDF viewer handles the bytes and never an HTML/SVG
    // document with a .pdf name. The frame must not be sandboxed: Chrome's PDF viewer refuses that.
    pdfBlob = new Blob([bytes], { type: 'application/pdf' });
    pdfUrl = URL.createObjectURL(pdfBlob);
    const frame = el('iframe', 'viewer-pdf');
    frame.title = `PDF: ${name}`;
    frame.src = pdfUrl;
    setBody(frame, 'pdf');
    ui.openTab.hidden = false;
  }

  async function open(path) {
    const k = kind(path);
    if (!deps || !k) return;
    ui ??= build();
    reset();
    const id = request;
    const name = path.split('/').pop();
    current = { path, kind: k };
    ui.title.textContent = name;
    ui.title.title = path;
    setMessage('Loading…');
    if (!ui.dialog.open) ui.dialog.showModal();
    try {
      const res = await deps.fetchFile(path);
      if (id !== request) return;
      await (k === 'pdf' ? showPdf(res, id, name) : showMarkdown(res, id));
    } catch (e) {
      if (id === request) setMessage(e?.message || 'Could not load the file.');
    }
  }

  function init(helpers) {
    deps = helpers;
    window.addEventListener('hashchange', hide); // another conversation is being opened
  }

  return { init, kind, open, close: hide, viewButton };
})();

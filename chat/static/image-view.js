'use strict';

// Images the agent produces, shared by both UIs (/v1 and /v2). A chart from generate_chart, a
// picture written by write_file or download_file, an image run_python created, or one the agent
// refers to in its text as ![caption](chart.png) is shown in the response, not only offered as a
// download. Each app hands over its own helpers through cedarImages.init():
//
//   fetchFile(path) -> Promise<Response>  an ok response for a workspace file (sends the access token)
//   open(path)                            shows the file in the viewer (optional)
//
// The files are fetched with the Authorization header and shown from a blob: URL, because the API
// takes its token from that header only. The blob's type is forced from the file's extension, so
// the browser treats the bytes as that kind of image and nothing else; an <img> never runs the
// scripts in an SVG or loads what it references. Tool output and the model's text are untrusted,
// so a path is only used once it is a plain relative workspace path.

const cedarImages = (() => {
  const TYPES = {
    png: 'image/png', jpg: 'image/jpeg', jpeg: 'image/jpeg', gif: 'image/gif', webp: 'image/webp',
    bmp: 'image/bmp', avif: 'image/avif', svg: 'image/svg+xml',
  };
  /** Larger files are left to the download button. */
  const MAX_BYTES = 16 * 1024 * 1024;
  /** How many of one tool call's images are shown inline; the rest are in the files panel. */
  const MAX_PER_CALL = 8;
  /** Tools whose `path` input is the file they write. */
  const FILE_TOOLS = new Set(['write_file', 'generate_chart', 'download_file']);
  const CHANGED_HEADING = '\n[files created or modified]\n';

  let deps = null;
  let cache = new Map(); // workspace path -> Promise<blob: URL>
  let urls = [];         // every blob: URL made for this conversation, revoked when it is left
  let generation = 0;    // bumped when the conversation changes, so a late fetch is dropped
  let observer = null;   // loads a figure's image once it comes near the viewport

  /** A path relative to the workspace, or null if `path` points elsewhere. */
  function workspacePath(path) {
    if (typeof path !== 'string') return null;
    let p = path.trim();
    if (p.startsWith('/workspace/')) p = p.slice('/workspace/'.length);
    else if (p.startsWith('./')) p = p.slice(2);
    if (!p || p.startsWith('/') || p.split('/').some((part) => !part || part === '.' || part === '..')) return null;
    return p;
  }

  /** The image type for a file name, or null when it isn't an image this module shows. */
  function type(path) {
    if (typeof path !== 'string') return null;
    const ext = path.slice(path.lastIndexOf('.') + 1).toLowerCase();
    return path.includes('.') && Object.hasOwn(TYPES, ext) ? TYPES[ext] : null;
  }

  const isImage = (path) => type(path) !== null;

  function el(tag, className, text) {
    const e = document.createElement(tag);
    if (className) e.className = className;
    if (text !== undefined) e.textContent = text;
    return e;
  }

  /** Forget every image: drop pending fetches and free the blob: URLs. */
  function reset() {
    generation++;
    for (const url of urls) URL.revokeObjectURL(url);
    urls = [];
    cache = new Map();
  }

  /** The file at `path` was written again: the next figure for it fetches it anew. */
  function forget(path) {
    const rel = workspacePath(path);
    if (rel) cache.delete(rel);
  }

  /** A blob: URL for the workspace image `rel`, fetched once per conversation. */
  function load(rel) {
    if (!deps) return Promise.reject(new Error('Images are not set up.'));
    const mime = type(rel);
    if (!mime) return Promise.reject(new Error('Not an image.'));
    let pending = cache.get(rel);
    if (!pending) {
      const gen = generation;
      pending = (async () => {
        const res = await deps.fetchFile(rel);
        if (Number(res.headers.get('Content-Length')) > MAX_BYTES) throw new Error('This image is too large to show here.');
        const bytes = await res.arrayBuffer();
        if (bytes.byteLength > MAX_BYTES) throw new Error('This image is too large to show here.');
        if (gen !== generation) throw new Error('The conversation changed.');
        const url = URL.createObjectURL(new Blob([bytes], { type: mime }));
        urls.push(url);
        return url;
      })();
      cache.set(rel, pending);
      pending.catch(() => { if (cache.get(rel) === pending) cache.delete(rel); });
    }
    return pending;
  }

  function watch(figure) {
    if (typeof IntersectionObserver === 'undefined') { fill(figure); return; }
    observer ??= new IntersectionObserver((entries) => {
      for (const entry of entries) {
        if (!entry.isIntersecting) continue;
        observer.unobserve(entry.target);
        fill(entry.target);
      }
    }, { rootMargin: '240px' });
    observer.observe(figure);
  }

  /** Fetch a figure's image and put it in place of the placeholder. */
  async function fill(figure) {
    const { path, alt } = figure.cedarImage;
    const frame = figure.querySelector('.cedar-image-frame');
    const gen = generation;
    let url;
    try {
      url = await load(path);
    } catch (e) {
      if (gen !== generation) return;
      figure.classList.add('failed');
      frame.replaceChildren(el('span', 'cedar-image-note', `${path.split('/').pop()}: ${e?.message || 'could not be shown'}`));
      return;
    }
    const img = el('img', 'cedar-image-img');
    img.alt = alt;
    img.decoding = 'async';
    img.addEventListener('load', () => {
      figure.classList.add('loaded');
      figure.cedarImage.onLoad?.(figure);
    });
    img.addEventListener('error', () => {
      figure.classList.add('failed');
      frame.replaceChildren(el('span', 'cedar-image-note', `${path.split('/').pop()}: the file is not a valid image.`));
    });
    if (deps.open) {
      img.title = `View ${path}`;
      img.tabIndex = 0;
      img.setAttribute('role', 'button');
      img.addEventListener('click', () => deps.open(path));
      img.addEventListener('keydown', (e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); deps.open(path); } });
    }
    img.src = url;
    frame.replaceChildren(img);
  }

  /**
   * A figure that shows the workspace image at `path` (loaded once it scrolls into view), with
   * its file name as the caption. `alt` is the model's caption, if it gave one; `onLoad` is
   * called once the picture is on screen (an app may want to keep the view scrolled to the end).
   */
  function figure(path, { alt, caption, onLoad } = {}) {
    const rel = workspacePath(path);
    const figureEl = el('figure', 'cedar-image');
    if (!rel || !type(rel)) {
      figureEl.classList.add('failed');
      figureEl.append(el('span', 'cedar-image-note', 'Not a workspace image.'));
      return figureEl;
    }
    const name = rel.split('/').pop();
    figureEl.cedarImage = { path: rel, alt: alt || name, onLoad };
    const frame = el('div', 'cedar-image-frame');
    frame.append(el('span', 'cedar-image-note', `Loading ${name}…`));
    const figcaption = el('figcaption', 'cedar-image-caption');
    figcaption.append(el('bdi', '', caption ?? (alt && alt !== name ? `${alt} · ${name}` : name)));
    figcaption.title = rel;
    figureEl.append(frame, figcaption);
    watch(figureEl);
    return figureEl;
  }

  /**
   * The workspace images a finished tool call produced, from its input and output: the file a
   * writing tool saved, or what run_python lists under "files created or modified".
   */
  function produced(name, input, output) {
    const paths = [];
    if (FILE_TOOLS.has(name)) {
      if (typeof input?.path === 'string') paths.push(input.path);
    } else if (name === 'run_python') {
      const text = String(output ?? '');
      const at = text.indexOf(CHANGED_HEADING);
      if (at >= 0) {
        for (const line of text.slice(at + CHANGED_HEADING.length).split('\n')) {
          if (!line.startsWith('- ')) break;
          paths.push(line.slice(2));
        }
      }
    }
    const seen = new Set();
    const images = [];
    for (const p of paths) {
      const rel = workspacePath(p);
      if (rel && type(rel) && !seen.has(rel)) { seen.add(rel); images.push(rel); }
    }
    return images;
  }

  /**
   * Figures for the images a finished tool call produced (see `produced`), or an empty list. At
   * most MAX_PER_CALL are shown; a note counts the rest.
   */
  function figuresFor(name, input, output, options) {
    const images = produced(name, input, output);
    for (const rel of images) forget(rel); // the call just (re)wrote them, so a cached copy is stale
    const nodes = images.slice(0, MAX_PER_CALL).map((rel) => figure(rel, options));
    if (images.length > MAX_PER_CALL) {
      const more = images.length - MAX_PER_CALL;
      nodes.push(el('div', 'cedar-image-more', `${more} more image${more === 1 ? '' : 's'} in the workspace files.`));
    }
    return nodes;
  }

  const escapeHtml = (s) => String(s).replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c]);

  /**
   * For a Markdown renderer: the HTML for an image reference `![alt](href)` when `href` is a
   * workspace image, else null (the renderer then leaves the text as it is). The HTML is a
   * placeholder that `hydrate` turns into the picture; left alone, it reads as the caption.
   */
  function markdown(alt, href) {
    const rel = workspacePath(href);
    if (!rel || !type(rel)) return null;
    return `<span class="cedar-image-ref" data-cedar-image="${escapeHtml(rel)}">${escapeHtml(alt || rel)}</span>`;
  }

  /** Replace the image placeholders a Markdown renderer left under `root` with figures. */
  function hydrate(root, options) {
    if (!deps || !root?.querySelectorAll) return;
    for (const ref of root.querySelectorAll('.cedar-image-ref[data-cedar-image]')) {
      const fig = figure(ref.dataset.cedarImage, { alt: ref.textContent, ...options });
      // A paragraph can't hold a figure, so one that only holds the image gives way to it.
      const p = ref.parentElement;
      if (p?.tagName === 'P' && p.childNodes.length === 1) p.replaceWith(fig);
      else ref.replaceWith(fig);
    }
  }

  function init(helpers) {
    deps = helpers;
    window.addEventListener('hashchange', reset); // another conversation is being opened
  }

  return { init, reset, forget, type, isImage, workspacePath, figure, produced, figuresFor, markdown, hydrate };
})();

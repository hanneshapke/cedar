'use strict';

// The tour film on index.html. The page shows a picture of it with a play button; a click puts
// the YouTube player in the picture's place and starts it, with sound. Until then nothing is
// fetched from YouTube, and the player comes from youtube-nocookie.com, which sets no cookies
// before the film plays. The film's id is on the frame (data-video), set per language by i18n.js.
//
//   [data-tour-frame]   the 16:9 box the picture sits in and the player goes into
//   [data-tour]         the picture's link (to the film on YouTube, for when scripts are off)
//   [data-tour-open]    a link elsewhere on the page that scrolls to the film and starts it
//
// It also starts the hero's short silent film, video[data-hero-loop]: muted is set here because
// a page's script may not carry the attribute over, and without it browsers refuse to autoplay.
// Visitors who ask their system for less motion get the still picture with the player's controls.

function startTour(frame) {
  if (frame.querySelector('iframe')) return;
  const lang = typeof cedarSite === 'object' ? cedarSite.lang : 'en';
  const query = new URLSearchParams({ autoplay: '1', rel: '0', hl: lang, cc_lang_pref: lang });
  const player = document.createElement('iframe');
  player.src = `https://www.youtube-nocookie.com/embed/${encodeURIComponent(frame.dataset.video)}?${query}`;
  player.title = frame.querySelector('[data-tour]')?.getAttribute('aria-label') || 'Cedar';
  player.allow = 'autoplay; encrypted-media; picture-in-picture; fullscreen';
  player.allowFullscreen = true;
  player.referrerPolicy = 'strict-origin-when-cross-origin';
  player.style.cssText = 'position:absolute;inset:0;width:100%;height:100%;border:0';
  frame.replaceChildren(player);
  player.focus();
}

document.addEventListener('click', (event) => {
  // a click meant for a new tab or window goes to YouTube as the link says
  if (event.defaultPrevented || event.button || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
  // the hero's film with its controls showing (less motion asked for, or autoplay refused):
  // a click there is for the controls, not for the link around it
  if (event.target.closest?.('video[controls]')) { event.preventDefault(); return; }
  const link = event.target.closest?.('[data-tour], [data-tour-open]');
  const frame = link && document.querySelector('[data-tour-frame]');
  if (!frame?.dataset.video) return;
  event.preventDefault();
  if (link.matches('[data-tour-open]')) frame.scrollIntoView({ behavior: 'smooth', block: 'center' });
  startTour(frame);
});

// Remembered here, not on the element: the page's template is read from the document and would
// copy a mark left on its video to the one it puts in.
const startedLoops = new WeakSet();

function startHeroLoop() {
  for (const film of document.querySelectorAll('video[data-hero-loop]')) {
    if (startedLoops.has(film)) continue;
    startedLoops.add(film);
    film.muted = true;
    film.defaultMuted = true;
    film.loop = true;
    film.playsInline = true;
    if (matchMedia('(prefers-reduced-motion: reduce)').matches) {
      film.controls = true;
      continue;
    }
    film.play().catch(() => { film.controls = true; });   // autoplay refused: let the visitor start it
  }
}

// The page's template is put in after this script has run, so wait for the film to appear.
new MutationObserver(startHeroLoop).observe(document.documentElement, { childList: true, subtree: true });
document.addEventListener('DOMContentLoaded', startHeroLoop);

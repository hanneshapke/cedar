'use strict';

// The More menu (⋯) in the top bar, shared by every UI. It holds the actions that don't need to
// be one click away: Rename, Settings, the run's details and Delete conversation. Markup:
//
//   <div class="more" data-more>
//     <button aria-haspopup="menu" aria-expanded="false" aria-controls="more-menu" data-more-toggle>…</button>
//     <div id="more-menu" role="menu" hidden>
//       <button role="menuitem">…</button> …
//     </div>
//   </div>
//
// Items keep their own ids and click handlers (#settings-btn, #delete-chat), so the code that
// opens Settings or deletes a conversation finds them as before; choosing one closes the menu.
// The menu closes on a click outside, Escape (focus goes back to ⋯) and Tab, and moves between
// items with the arrow keys, Home and End. Each UI styles it in its own style.css.

const cedarMore = (() => {
  let open = null; // { root, toggle, menu } while a menu is open

  function items(menu) {
    return [...menu.querySelectorAll('[role="menuitem"]')].filter((b) => !b.hidden && !b.disabled);
  }

  function show(root, toggle, menu) {
    close(false);
    menu.hidden = false;
    toggle.setAttribute('aria-expanded', 'true');
    open = { root, toggle, menu };
    items(menu)[0]?.focus();
    document.addEventListener('pointerdown', onOutside, true);
    document.addEventListener('keydown', onKey, true);
  }

  function close(restoreFocus) {
    if (!open) return;
    const { toggle, menu } = open;
    open = null;
    menu.hidden = true;
    toggle.setAttribute('aria-expanded', 'false');
    document.removeEventListener('pointerdown', onOutside, true);
    document.removeEventListener('keydown', onKey, true);
    if (restoreFocus) toggle.focus();
  }

  function onOutside(e) {
    if (open && !open.root.contains(e.target)) close(false);
  }

  function onKey(e) {
    if (!open) return;
    if (e.key === 'Escape') { e.preventDefault(); close(true); return; }
    if (e.key === 'Tab') { close(false); return; }
    if (!['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(e.key)) return;
    e.preventDefault();
    const list = items(open.menu);
    if (!list.length) return;
    const i = list.indexOf(document.activeElement);
    const step = e.key === 'ArrowDown' ? 1 : -1;
    const next = e.key === 'Home' ? 0 : e.key === 'End' ? list.length - 1
      : i < 0 ? (step > 0 ? 0 : list.length - 1) : (i + step + list.length) % list.length;
    list[next].focus();
  }

  function bind() {
    for (const root of document.querySelectorAll('[data-more]')) {
      const toggle = root.querySelector('[data-more-toggle]');
      const menu = root.querySelector('[role="menu"]');
      if (!toggle || !menu) continue;
      toggle.addEventListener('click', () => (open?.menu === menu ? close(true) : show(root, toggle, menu)));
      // Any item closes the menu; its own handler does the rest.
      menu.addEventListener('click', (e) => { if (e.target.closest('[role="menuitem"]')) close(false); });
    }
  }

  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', bind);
  else bind();

  return { close };
})();

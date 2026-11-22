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
//
// A menu whose only content is an item marked data-more-solo (Settings) isn't worth a click to
// open: the ⋯ button then wears that item's icon and label and runs it directly. It turns back
// into ⋯ as soon as another item or some run details show up in the menu.

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

  // The item the toggle stands in for, or null when the menu has more to show than one solo item.
  function soloItem(menu) {
    const list = items(menu);
    if (list.length !== 1 || !list[0].hasAttribute('data-more-solo')) return null;
    const extra = [...menu.children].some((c) => c !== list[0] && c.getAttribute('role') !== 'menuitem'
      && !c.hidden && c.textContent.trim() !== '');
    return extra ? null : list[0];
  }

  // Swap the toggle between ⋯ (opens the menu) and the solo item's own icon and label.
  function sync(toggle, menu, saved) {
    const solo = soloItem(menu);
    if (solo) {
      if (open?.menu === menu) close(false);
      const icon = solo.querySelector('svg')?.cloneNode(true);
      const label = solo.textContent.trim();
      if (!saved.solo) {
        saved.solo = true;
        toggle.removeAttribute('aria-haspopup');
        toggle.removeAttribute('aria-expanded');
        // i18n.js would put "More" back on a language change; the label follows the item instead.
        if (saved.i18n != null) toggle.removeAttribute('data-i18n-attr');
      }
      if (icon) toggle.replaceChildren(icon);
      toggle.setAttribute('aria-label', label);
      toggle.title = label;
    } else if (saved.solo) {
      saved.solo = false;
      toggle.replaceChildren(...saved.nodes.map((n) => n.cloneNode(true)));
      toggle.setAttribute('aria-haspopup', 'menu');
      toggle.setAttribute('aria-expanded', 'false');
      toggle.setAttribute('aria-label', saved.label);
      toggle.title = saved.title;
      if (saved.i18n != null) {
        toggle.setAttribute('data-i18n-attr', saved.i18n);
        if (typeof cedarI18n !== 'undefined') cedarI18n.apply?.(toggle.parentNode);
      }
    }
  }

  function bind() {
    for (const root of document.querySelectorAll('[data-more]')) {
      const toggle = root.querySelector('[data-more-toggle]');
      const menu = root.querySelector('[role="menu"]');
      if (!toggle || !menu) continue;
      const saved = {
        solo: false,
        nodes: [...toggle.childNodes].map((n) => n.cloneNode(true)),
        label: toggle.getAttribute('aria-label') ?? '',
        title: toggle.title,
        i18n: toggle.getAttribute('data-i18n-attr'),
      };
      toggle.addEventListener('click', () => {
        if (saved.solo) { soloItem(menu)?.click(); return; }
        open?.menu === menu ? close(true) : show(root, toggle, menu);
      });
      // Re-check whenever an item is shown or hidden, or the details or labels change.
      new MutationObserver(() => sync(toggle, menu, saved)).observe(menu,
        { subtree: true, childList: true, characterData: true, attributes: true, attributeFilter: ['hidden', 'disabled'] });
      sync(toggle, menu, saved);
      // Any item closes the menu; its own handler does the rest.
      menu.addEventListener('click', (e) => { if (e.target.closest('[role="menuitem"]')) close(false); });
    }
  }

  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', bind);
  else bind();

  return { close };
})();

'use strict';

// The service worker behind Web Push (Settings → Notifications → Browser). It only shows the
// messages the chat server sends and opens the conversation when one is clicked; it caches
// nothing and handles no fetches, so the pages work as before. A message that arrives while a
// Cedar tab is in front is left to that tab, which shows it in the inbox and as a toast.

self.addEventListener('install', () => self.skipWaiting());
self.addEventListener('activate', (event) => event.waitUntil(self.clients.claim()));

self.addEventListener('push', (event) => {
  let data = {};
  try { data = event.data ? event.data.json() : {}; } catch { data = { body: event.data?.text() }; }
  const title = data.title || 'Cedar';
  const options = {
    body: data.body || '',
    tag: data.tag || undefined,
    data: { url: data.url || '/' },
    icon: '/assets/apple-touch-icon.png',
    badge: '/assets/favicon-32.png',
  };
  event.waitUntil((async () => {
    const windows = await self.clients.matchAll({ type: 'window', includeUncontrolled: true });
    if (windows.some((w) => w.focused)) return; // the tab in front shows it itself
    await self.registration.showNotification(title, options);
  })());
});

self.addEventListener('notificationclick', (event) => {
  event.notification.close();
  const url = event.notification.data?.url || '/';
  event.waitUntil((async () => {
    const windows = await self.clients.matchAll({ type: 'window', includeUncontrolled: true });
    const same = windows.find((w) => w.url.startsWith(self.registration.scope));
    if (same) {
      await same.focus();
      // The pages route on the hash; tell the tab where to go.
      if ('navigate' in same) await same.navigate(url).catch(() => {});
      return;
    }
    await self.clients.openWindow(url);
  })());
});

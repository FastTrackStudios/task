// Task Architect service worker.
//
// Strategy: cache-first for static assets, network-only for the
// vox WebSocket (`/sync`) — anything that's not a same-origin GET
// passes straight through to the network without touching cache.
//
// Cache version bumps on every release so old caches can't pin
// stale wasm. Bump CACHE_VERSION when shipping a breaking change
// to the asset bundle (rare; the asset URLs are content-hashed by
// Dioxus's asset! macro so most changes naturally invalidate).

const CACHE_VERSION = 'task-arch-v2';
const SHELL = ['/', '/manifest.json'];

// Inline fallback page served when the user is offline AND the
// requested resource isn't in the cache. Self-contained so it
// doesn't itself rely on assets that might be missing.
const OFFLINE_HTML = `<!DOCTYPE html>
<html lang="en"><head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Offline — Task Architect</title>
<style>
  body { font-family: system-ui, sans-serif; background: #0a0a0a; color: #e8e8e8;
    display: flex; align-items: center; justify-content: center; min-height: 100vh; margin: 0; padding: 1rem; }
  .card { max-width: 32rem; text-align: center; }
  h1 { font-size: 1.5rem; margin: 0 0 0.5rem; }
  p { color: #888; line-height: 1.6; }
  button { background: #2563eb; color: white; border: none;
    padding: 0.5rem 1rem; border-radius: 0.25rem; font: inherit; cursor: pointer; margin-top: 1rem; }
  button:hover { background: #1d4ed8; }
</style>
</head><body>
<div class="card">
  <h1>You're offline</h1>
  <p>Task Architect is local-first, but this resource isn't in your cache yet.
     Open the app at least once while online to install the offline shell, then
     it'll work without a network.</p>
  <p>Your edits are still safe — they're saved to local IndexedDB and will sync
     to peers when the network returns.</p>
  <button onclick="location.reload()">Try again</button>
</div>
</body></html>`;

self.addEventListener('install', (event) => {
  event.waitUntil(
    caches.open(CACHE_VERSION).then((cache) => cache.addAll(SHELL))
      .then(() => self.skipWaiting())
  );
});

self.addEventListener('activate', (event) => {
  event.waitUntil(
    caches.keys()
      .then((keys) => Promise.all(
        keys.filter((k) => k !== CACHE_VERSION).map((k) => caches.delete(k))
      ))
      .then(() => self.clients.claim())
  );
});

// Background sync handler. The Page registers a `task-arch-sync`
// tag whenever local IDB has un-published updates and the
// network looks down; the browser fires this `sync` event when
// connectivity returns (even if the tab is closed). The handler
// just opens the SPA so its sync loop can do the actual upload —
// the SW doesn't have direct IDB cursor access without
// duplicating the schema knowledge.
self.addEventListener('sync', (event) => {
  if (event.tag === 'task-arch-sync') {
    event.waitUntil(
      // Best-effort: try to open a window that has our origin
      // already loaded. The tab's existing sync loop handles the
      // actual replay from IDB.
      self.clients.matchAll({ type: 'window' }).then((clients) => {
        if (clients.length > 0) {
          // Tab is open — nothing to do; its in-tab loop will
          // catch the network restoration via the existing
          // backoff retry.
          return;
        }
        // No tabs open — best we can do is queue the user to
        // visit the app. (Browsers don't let SWs spawn windows
        // without user gesture.) Logged for diagnostics.
        console.log('task-arch-sync fired with no open clients');
      })
    );
  }
});

self.addEventListener('fetch', (event) => {
  const req = event.request;
  // Only handle same-origin GETs. Everything else (WebSocket
  // upgrades, POSTs, cross-origin requests) goes straight to
  // network without caching.
  if (req.method !== 'GET') return;
  const url = new URL(req.url);
  if (url.origin !== self.location.origin) return;

  // Cache-first for assets + shell; network falls back so dev
  // reloads pick up changes immediately.
  event.respondWith(
    caches.match(req).then((cached) => {
      if (cached) return cached;
      return fetch(req).then((res) => {
        // Only cache successful, basic (same-origin) responses.
        if (!res || res.status !== 200 || res.type !== 'basic') return res;
        const clone = res.clone();
        caches.open(CACHE_VERSION).then((cache) => cache.put(req, clone));
        return res;
      }).catch(() => {
        // Offline + not in cache. For navigation requests show
        // either the cached SPA shell (if installed) or the
        // inline fallback page so the user knows what's happening.
        if (req.mode === 'navigate') {
          return caches.match('/').then((cached) =>
            cached || new Response(OFFLINE_HTML, {
              status: 200,
              headers: { 'Content-Type': 'text/html; charset=utf-8' },
            })
          );
        }
        return new Response('offline', { status: 504, statusText: 'Offline' });
      });
    })
  );
});

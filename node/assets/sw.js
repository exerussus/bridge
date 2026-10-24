// Мост на телефоне: страница открывается и без связи (покажет, что ПК недоступен), а «Поделиться» из других приложений
// кладёт файлы сюда — страница заберёт их и спросит, куда отправить.
const SHELL = 'bridge-m-shell-v1';
self.addEventListener('install', e => { self.skipWaiting(); e.waitUntil(caches.open(SHELL).then(c => c.addAll(['/m/', '/m/icon-192.png'])).catch(() => {})); });
self.addEventListener('activate', e => e.waitUntil((async () => {
  for (const k of await caches.keys()) if (k.startsWith('bridge-m-shell-') && k !== SHELL) await caches.delete(k);
  await self.clients.claim();
})()));
self.addEventListener('fetch', e => {
  const u = new URL(e.request.url);
  if (u.origin !== location.origin) return;
  if (e.request.method === 'POST' && u.pathname === '/m/share') { e.respondWith(share(e.request)); return; }
  if (e.request.method === 'GET' && (u.pathname === '/m/' || u.pathname === '/m/index.html')) {
    e.respondWith(fetch(e.request).then(r => { if (r.ok) { const c = r.clone(); caches.open(SHELL).then(x => x.put('/m/', c)); } return r; }).catch(() => caches.match('/m/')));
  }
});
async function share(req){
  try {
    const fd = await req.formData(); const c = await caches.open('bridge-share'); const stamp = Date.now(); let n = 0;
    for (const f of fd.getAll('files')) {
      if (!f || typeof f === 'string') continue;
      await c.put('/m/_share/' + stamp + '-' + (n++), new Response(f, { headers: { 'x-name': encodeURIComponent(f.name || 'файл'), 'content-type': f.type || 'application/octet-stream' } }));
    }
    const text = [fd.get('title'), fd.get('text'), fd.get('url')].filter(x => x && typeof x === 'string').join('\n').trim();
    if (text && !n) await c.put('/m/_share/' + stamp + '-text', new Response(text, { headers: { 'x-name': encodeURIComponent('текст.txt'), 'x-text': '1', 'content-type': 'text/plain' } }));
    return Response.redirect('/m/#shared', 303);
  } catch (e) {
    return Response.redirect('/m/#share-failed', 303);
  }
}

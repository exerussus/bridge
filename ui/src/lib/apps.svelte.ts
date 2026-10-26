// Приложения (3.4): список, открытые вкладки, обработчик вызовов из изолированных рамок (most.* → панель → мост).
// Состояние живёт в модуле: открытые приложения не перезагружаются при переходе на другие вкладки моста.
import { api, authHeaders, API, onMedia, upload } from './api';
import { SET, setSet, toast, ui, badgeCount } from './state.svelte';
import { pickFiles } from './util';
import { notifyTo } from './notify';

export type AppInfo = {
  id: string; name: string; version: string; description: string; author: string; packaged: boolean; source: string;
  ui: boolean; native: boolean; service: boolean; actions: any[]; icon: boolean; perms: string[]; granted: string[];
  state: 'new' | 'changed' | 'ok' | 'unsupported'; unsupported: string; window: any; net: any; opens: any; launch: any; dev: any;
  hash: string; uiStamp: string; entry: string; runningService: any; runningNative: any; lastExit: string;
};

export const PERM_LABEL: Record<string, string> = {
  'files.pick': 'Получать файлы, которые вы выберете',
  'files.save': 'Сохранять файлы туда, куда вы выберете',
  'files.read': 'Читать общие папки и принятые файлы',
  notify: 'Уведомления у часов',
  'channel.post': 'Писать в чат канала от вашего имени',
  peers: 'Видеть соседей и писать тому же приложению у них',
  'mail.send': 'Отправлять файлы соседям',
  sessions: 'Сетевые сеансы в канале',
  activity: 'Видеть вашу активность в мосте (без содержимого)',
  'input.capture': 'Захватывать мышь и клавиатуру в своём окне',
};

export const AP = $state({
  list: [] as AppInfo[],
  errors: [] as { source: string; error: string }[],
  base: '',
  folder: '',
  loaded: false,
  /** Открытые вкладки (id) и активная. */
  open: [] as string[],
  active: '',
  view: 'desk' as 'desk' | 'app',
  /** Заголовок и счётчик, которые выставило само приложение. */
  titles: {} as Record<string, string>,
  badges: {} as Record<string, string>,
  /** Номер перезагрузки рамки (меняется — рамка пересоздаётся). */
  gen: {} as Record<string, number>,
  /** Новая версия пакета — вкладка предлагает перезапустить. */
  stale: {} as Record<string, boolean>,
  popped: {} as Record<string, boolean>,
});

export const byId = (id: string) => AP.list.find(a => a.id === id);

export async function loadApps() {
  try {
    const r = await api('GET', 'apps');
    AP.list = r.list; AP.errors = r.errors; AP.base = r.base; AP.folder = r.folder; AP.loaded = true;
    // закрыть вкладки убранных приложений
    AP.open = AP.open.filter(id => byId(id));
    if (AP.active && !byId(AP.active)) { AP.active = AP.open[0] || ''; if (!AP.active) AP.view = 'desk'; }
    updateBadge();
  } catch {}
}

function updateBadge() {
  const waiting = AP.list.filter(a => a.state === 'new' || a.state === 'changed').length;
  badgeCount('apps', AP.open.length || waiting);
}

/** Порядок ярлыков: сохранённый, новые — в конец. */
export function ordered(): AppInfo[] {
  const order: string[] = SET.appsOrder || [];
  const idx = (id: string) => { const i = order.indexOf(id); return i < 0 ? 1e6 : i; };
  return [...AP.list].sort((a, b) => idx(a.id) - idx(b.id) || a.name.localeCompare(b.name, 'ru'));
}
export function reorder(from: string, to: string) {
  const ids = ordered().map(a => a.id).filter(x => x !== from);
  const i = ids.indexOf(to);
  ids.splice(i < 0 ? ids.length : i, 0, from);
  setSet('appsOrder', ids);
}

/** Адрес рамки: веб-интерфейс приложения на 127.0.0.1 (не через протокол окна — туда рамке нельзя). */
export function frameSrc(a: AppInfo): string {
  if (a.dev && typeof a.dev === 'string') return a.dev;
  return AP.base + '/apps/' + encodeURIComponent(a.id) + '/ui/' + entryOf(a) + '?v=' + encodeURIComponent(a.uiStamp);
}
function entryOf(a: AppInfo) { return a.entry || 'index.html'; }
export function sandboxOf(a: AppInfo): string {
  return 'allow-scripts allow-forms' + (a.granted.includes('input.capture') ? ' allow-pointer-lock' : '');
}
export function allowOf(a: AppInfo): string {
  return a.granted.includes('input.capture') ? 'fullscreen; clipboard-write' : 'clipboard-write';
}

export function openApp(id: string) {
  const a = byId(id); if (!a) return;
  if (!AP.open.includes(id)) AP.open.push(id);
  AP.active = id; AP.view = 'app';
  if (a.service) api('POST', 'apps/touch', { id }).catch(() => {});
  if (a.native && !a.runningNative) startNative(id);
  updateBadge();
}
export function closeApp(id: string) {
  AP.open = AP.open.filter(x => x !== id);
  delete AP.titles[id]; delete AP.badges[id]; delete AP.stale[id];
  if (AP.active === id) { AP.active = AP.open[AP.open.length - 1] || ''; if (!AP.active) AP.view = 'desk'; }
  for (const w of popups(id)) { try { w.close(); } catch {} }
  updateBadge();
}
export function reloadApp(id: string) { AP.gen[id] = (AP.gen[id] || 0) + 1; AP.stale[id] = false; for (const w of popups(id)) { try { (w as any).__appEvent('reload'); } catch {} } }

export async function startNative(id: string) {
  try { await api('POST', 'apps/start', { id, kind: 'native' }); await loadApps(); } catch (e: any) { toast(e.message, 'bad'); }
}

// ── отдельные окна ──
const POPS: Record<string, Set<Window>> = {};
const popups = (id: string) => [...(POPS[id] || [])].filter(w => !w.closed);
export function popout(id: string) {
  const a = byId(id); if (!a || !a.ui) return;
  const w = a.window || {};
  const ww = Math.max(320, Math.min(2400, +w.w || 480)), hh = Math.max(240, Math.min(1600, +w.h || 680));
  const win = window.open('/assets/apphost.html?id=' + encodeURIComponent(id), 'app-' + id, `popup,width=${ww},height=${hh}`);
  if (!win) { toast('Окно не открылось — разрешите всплывающие окна', 'bad'); return; }
  AP.popped[id] = true;
}

// ── вызовы из рамок ──

function need(a: AppInfo, perm: string) {
  if (!a.granted.includes(perm)) throw new Error('у приложения нет права «' + (PERM_LABEL[perm] || perm) + '»');
}

async function toBlob(data: any): Promise<Blob> {
  if (data instanceof Blob) return data;
  if (data instanceof ArrayBuffer || ArrayBuffer.isView(data)) return new Blob([data as any]);
  return new Blob([String(data ?? '')], { type: 'text/plain;charset=utf-8' });
}

/** Обработать вызов most.* от приложения (из вкладки или отдельного окна). */
// ── сеть приложений (3.5): у каждой рамки своё подключение к мосту, панель только пересылает ──
type Ctx = { key: any; emit: (ev: string, v: any) => void };
type NetConn = { ws: WebSocket; seq: number; pending: Map<number, { res: (v: any) => void; rej: (e: Error) => void }>; open: Promise<any> };
const NETS = new Map<any, NetConn>();
async function netFor(id: string, ctx: Ctx): Promise<NetConn> {
  const have = NETS.get(ctx.key);
  if (have && have.ws.readyState <= 1) { await have.open; return have; }
  const { url } = await api('POST', 'apps/net/ticket', { id });
  const ws = new WebSocket(url);
  ws.binaryType = 'arraybuffer';
  const nc: NetConn = { ws, seq: 0, pending: new Map(), open: null as any };
  nc.open = new Promise((res, rej) => {
    let hello = false;
    ws.onmessage = e => {
      if (typeof e.data === 'string') {
        let v: any; try { v = JSON.parse(e.data); } catch { return; }
        if (v.ev === 'hello' && !hello) { hello = true; res(v); return; }
        if (v.ev === 're') { const p = nc.pending.get(v.re); if (p) { nc.pending.delete(v.re); v.ok ? p.res(v.v) : p.rej(new Error(v.error || 'ошибка')); } return; }
        ctx.emit('net', v);
      } else {
        const b = e.data as ArrayBuffer, d = new DataView(b);
        if (b.byteLength < 3) return;
        ctx.emit('netdata', { delivery: d.getUint8(0), from: d.getUint16(1, true), data: b.slice(3) });
      }
    };
    ws.onerror = () => { if (!hello) rej(new Error('мост не принял подключение приложения')); };
    ws.onclose = () => { for (const p of nc.pending.values()) p.rej(new Error('связь с мостом закрыта')); nc.pending.clear(); if (NETS.get(ctx.key) === nc) NETS.delete(ctx.key); ctx.emit('net', { ev: 'closed' }); };
  });
  NETS.set(ctx.key, nc);
  await nc.open;
  return nc;
}
async function netOp(id: string, ctx: Ctx, op: any): Promise<any> {
  const nc = await netFor(id, ctx);
  const re = ++nc.seq;
  return new Promise((res, rej) => { nc.pending.set(re, { res, rej }); nc.ws.send(JSON.stringify({ ...op, re })); setTimeout(() => { if (nc.pending.delete(re)) rej(new Error('мост не ответил')); }, 15000); });
}
export function closeNet(key: any) { const nc = NETS.get(key); if (nc) { NETS.delete(key); try { nc.ws.close(); } catch {} } }
const DELIV: Record<string, number> = { reliable: 0, fast: 1, latest: 2 };

export async function handleCall(id: string, m: string, args: any[], ctx?: Ctx): Promise<any> {
  const a = byId(id);
  if (!a) throw new Error('приложение убрано');
  if (a.state !== 'ok') throw new Error('приложение не разрешено на этом ПК');
  switch (m) {
    case 'ready':
      return { id: a.id, name: a.name, version: a.version, node: ui.ov?.node?.node || '', theme: SET.theme || 'dark', granted: [...a.granted], bridge: 'apps@1' };
    case 'store.get': return (await api('GET', 'apps/store?' + new URLSearchParams({ id, key: String(args[0] || '') }))).value;
    case 'store.set': await api('POST', 'apps/store', { id, key: String(args[0] || ''), value: args[1] ?? null }); return true;
    case 'files.pick': {
      need(a, 'files.pick');
      const o = args[0] || {};
      return await pickFiles({ multiple: !!o.multiple, accept: typeof o.accept === 'string' ? o.accept : undefined });
    }
    case 'files.save': {
      need(a, 'files.save');
      const b = await toBlob(args[1]);
      const u = URL.createObjectURL(b), l = document.createElement('a');
      l.href = u; l.download = String(args[0] || 'file').replace(/[\\/:*?"<>|]/g, '_').slice(0, 120); l.style.display = 'none'; document.body.appendChild(l); l.click(); l.remove();
      setTimeout(() => URL.revokeObjectURL(u), 10000);
      return true;
    }
    case 'service.fetch': {
      if (!a.service) throw new Error('у приложения нет службы');
      const path = String(args[0] || '').replace(/^\/+/, ''), o = args[1] || {};
      const h: Record<string, string> = { ...authHeaders() };
      if (o.contentType) h['Content-Type'] = String(o.contentType);
      const method = String(o.method || 'GET').toUpperCase();
      const r = await fetch(API + 'apps/svc/' + encodeURIComponent(id) + '/' + path, { method, headers: h, body: method === 'GET' || method === 'HEAD' || o.body == null ? undefined : o.body });
      return { status: r.status, contentType: r.headers.get('content-type') || '', body: await r.arrayBuffer() };
    }
    case 'service.status': { await loadApps(); const x = byId(id); return { running: !!x?.runningService, ...(x?.runningService || {}), lastExit: x?.lastExit || '' }; }
    case 'service.restart':
      await api('POST', 'apps/stop', { id, kind: 'service' });
      await new Promise(r => setTimeout(r, 400));
      await api('POST', 'apps/start', { id, kind: 'service' }); loadApps(); return true;
    case 'notify': need(a, 'notify'); notifyTo(a.name, String(args[0] || '').slice(0, 300), 'apps'); return true;
    case 'toast': toast(a.name + ': ' + String(args[0] || '').slice(0, 300)); return true;
    case 'window.title': AP.titles[id] = String(args[0] || '').slice(0, 60); for (const w of popups(id)) { try { (w as any).__setTitle(AP.titles[id] || a.name); } catch {} } return true;
    case 'window.badge': AP.badges[id] = String(args[0] || '').slice(0, 6); return true;
    case 'window.popout': popout(id); return true;
    case 'net.connect': need(a, 'sessions'); if (!ctx) throw new Error('нет рамки'); return await netFor(id, ctx).then(nc => nc.open);
    case 'net.op': need(a, 'sessions'); if (!ctx) throw new Error('нет рамки'); return await netOp(id, ctx, args[0] || {});
    case 'net.send': {
      need(a, 'sessions'); if (!ctx) throw new Error('нет рамки');
      const nc = await netFor(id, ctx);
      const dl = DELIV[String(args[1] || 'reliable')] ?? 0, to = (+args[0]) & 0xffff, key = (+args[3] || 0) & 0xff;
      let body: Uint8Array;
      const d = args[2];
      if (d instanceof ArrayBuffer) body = new Uint8Array(d); else if (ArrayBuffer.isView(d)) body = new Uint8Array(d.buffer, d.byteOffset, d.byteLength); else body = new TextEncoder().encode(String(d ?? ''));
      const head = dl === 2 ? 4 : 3, f = new Uint8Array(head + body.byteLength);
      f[0] = dl; f[1] = to & 0xff; f[2] = to >> 8; if (dl === 2) f[3] = key; f.set(body, head);
      nc.ws.send(f);
      return true;
    }
    case 'net.close': if (ctx) closeNet(ctx.key); return true;
    case 'activity.since': need(a, 'activity'); return await api('GET', 'apps/activity?' + new URLSearchParams({ since: String(+args[0] || 0), limit: String(+args[1] || 500) }));
    case 'activity.stats': need(a, 'activity'); return await api('GET', 'apps/activity?stats=' + encodeURIComponent(String(args[0] || 'today')));
  }
  throw new Error('нет такого вызова: ' + m);
}

/** Ответ без реактивных обёрток (их нельзя передать в другое окно); файлы и буферы — как есть. */
function plain(v: any): any {
  if (v == null || typeof v !== 'object' || v instanceof Blob || v instanceof ArrayBuffer || ArrayBuffer.isView(v)) return v;
  if (Array.isArray(v)) return v.map(plain);
  const o: any = {}; for (const k of Object.keys(v)) o[k] = plain(v[k]); return o;
}

/** Рамка вкладки прислала сообщение. */
export function onFrameMessage(id: string, frame: HTMLIFrameElement, e: MessageEvent) {
  if (e.source !== frame.contentWindow) return;
  const d = e.data; if (!d || d.most !== 1 || !d.id) return;
  const ctx: Ctx = { key: frame, emit: (ev, v) => { try { frame.contentWindow?.postMessage({ most: 1, ev, v }, '*'); } catch {} } };
  handleCall(id, String(d.m || ''), Array.isArray(d.a) ? d.a : [], ctx).then(
    v => frame.contentWindow?.postMessage({ most: 1, re: d.id, v: plain(v) }, '*'),
    err => frame.contentWindow?.postMessage({ most: 1, re: d.id, err: String(err?.message || err) }, '*'),
  );
}

/** Событие во все рамки приложения (вкладка и окна). */
export function emitTo(id: string, ev: string, v: any) {
  document.querySelectorAll<HTMLIFrameElement>('iframe.appframe[data-app="' + CSS.escape(id) + '"]').forEach(f => { try { f.contentWindow?.postMessage({ most: 1, ev, v }, '*'); } catch {} });
  for (const w of popups(id)) { try { (w as any).__appEvent(ev, v); } catch {} }
}

// отдельные окна (apphost.html) берут сведения и делают вызовы через панель
(window as any).__apps = {
  info: (id: string) => { const a = byId(id); return a && a.ui && a.state === 'ok' ? { name: a.name, src: frameSrc(a), sandbox: sandboxOf(a), allow: allowOf(a) } : null; },
  call: (id: string, m: string, a: any[], w?: Window) => handleCall(id, m, a, w ? { key: w, emit: (ev: string, v: any) => { try { (w as any).__appEvent(ev, v); } catch {} } } : undefined).then(plain),
  attach: (id: string, w: Window) => { (POPS[id] = POPS[id] || new Set()).add(w); AP.popped[id] = true; },
  detach: (id: string, w: Window) => { POPS[id]?.delete(w); closeNet(w); AP.popped[id] = popups(id).length > 0; },
};

/** Перетащили пакет .most-app — положить в папку приложений. */
export async function importPackages(files: File[]) {
  for (const f of files) {
    if (!/\.most-app$/i.test(f.name)) { toast('Это не пакет приложения (.most-app): ' + f.name, 'bad'); continue; }
    try { await upload('apps/import?name=' + encodeURIComponent(f.name), f); toast('Приложение «' + f.name + '» добавлено — разрешите его на ярлыке', 'ok'); }
    catch (e: any) { toast(e.message, 'bad'); }
  }
  setTimeout(loadApps, 2500);
}

// сигналы моста: список поменялся, файлы интерфейса обновились, уведомления служб
let started = false;
export function startApps() {
  if (started) return; started = true;
  loadApps();
  onMedia(m => {
    if (m.media !== 'apps') return;
    if (m.type === 'changed') loadApps();
    else if (m.type === 'reload' && m.id) {
      const a = byId(m.id);
      if (!AP.open.includes(m.id) && !AP.popped[m.id]) return;
      if (a && !a.packaged) { loadApps().then(() => reloadApp(m.id)); } else { AP.stale[m.id] = true; loadApps(); }
    } else if (m.type === 'notify') notifyTo(m.name || 'Приложение', m.text || '', 'apps');
    else if (m.type === 'toast') toast((m.name ? m.name + ': ' : '') + (m.text || ''));
    else if (m.type === 'activity' && m.e) { for (const id of new Set([...AP.open, ...Object.keys(POPS)])) if (byId(id)?.granted.includes('activity')) emitTo(id, 'activity', m.e); }
    else if (m.type === 'dev' && m.req) devAsk(m);
  });
  // открытые приложения со службой держат её живой
  setInterval(() => { for (const id of AP.open) if (byId(id)?.service) api('POST', 'apps/touch', { id }).catch(() => {}); for (const id of Object.keys(POPS)) if (popups(id).length && byId(id)?.service) api('POST', 'apps/touch', { id }).catch(() => {}); }, 20000);
}

// редактор (Unity и т. п.) просит подключиться как приложение — решает хозяин ПК
const asked = new Set<string>();
async function devAsk(m: any) {
  if (asked.has(m.req)) return; asked.add(m.req);
  notifyTo('Мост', (m.who || 'Редактор') + ' просит подключиться как «' + m.name + '»', 'apps');
  const ok = confirm((m.who || 'Редактор') + ' просит подключиться к мосту как приложение «' + m.name + '».\n\nРазрешить? Подключение получит права приложения до перезапуска моста.');
  try { await api('POST', 'apps/dev/answer', { req: m.req, ok }); toast(ok ? 'Редактор подключён как «' + m.name + '»' : 'Отказано', ok ? 'ok' : ''); } catch (e: any) { toast(e.message, 'bad'); }
}

/** Что идёт в моём канале (для полосы «Идёт сейчас»). */
export async function loadSessions(): Promise<any> { try { return await api('GET', 'apps/net'); } catch { return null; } }

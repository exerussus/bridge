// ═════════════ Показ экрана: указка, рисование поверх и управление ПК ═════════════
// Зрители могут пинговать точку на чужом экране и рисовать поверх — это видят все, кто смотрит показ, а у
// показывающего (Windows, показ всего экрана) — прямо на его экране (окно-наложение, которое в показ не попадает).
// Управление ПК — только по запросу и только после подтверждения показывающим: кнопку разрешения нужно удерживать
// 2 секунды (случайно не нажать), «Отклонить» — сразу. Разрешение живёт, пока панель его продлевает.
import { api, aurl } from '../lib/api';
import { toast, titleOf, me as ME, SET, setSet } from '../lib/state.svelte';
import { ring } from '../lib/audio';
import { R, rsend, rosterOf, reachable, SESSION, onRoomMsg, onUiMsg, onLeave, onShareStop, addUi, watch } from './engine.svelte';

const COLS = ['#38E1F0', '#FFB547', '#5DE08E', '#FF6B7D', '#9B7BFF', '#F5E26B', '#7FD1FF'];
export const colOf = (n: string) => { let h = 0; for (const ch of n) h = (h * 31 + ch.charCodeAt(0)) >>> 0; return COLS[h % COLS.length]; };

/** Режим инструмента на каждом показе ('ping' | 'draw' | 'ctl') и «я управляю чужим ПК». */
export const SX = $state({
  mode: {} as Record<string, string>,
  cv: { stream: '', peer: '', keyboard: false },
  /** Мной управляют: {peer, keyboard, stream, since}. */
  ch: null as null | { peer: string; keyboard: boolean; stream: string; since: number },
  /** Запрос управления, ждёт решения: {n, stream, keyboard}. */
  ask: null as null | { n: string; stream: string; keyboard: boolean },
});
const AN: { items: Record<string, any[]>; raf: number } = { items: {}, raf: 0 };
const CVQ: { q: any[]; busy: boolean } = { q: [], busy: false };

export function streamOf(id: string) {
  for (const p of rosterOf(R.key)) for (const s of p.st.streams || []) if (s.id === id) return { n: p.name, ...s };
  return null;
}
export function setMode(id: string, mode: string) {
  SX.mode[id] = SX.mode[id] === mode ? '' : mode;
  if (SX.mode[id] === 'ctl' && SX.cv.stream !== id) SX.mode[id] = '';
  if (SX.mode[id] === 'ctl') (document.querySelector('[data-stream="' + CSS.escape(id) + '"] canvas.ann') as HTMLElement | null)?.focus();
}
export function clearMine(id: string) { const s = streamOf(id); if (s) ann(s, { k: 'clear' }); }

// ── наложение на видео: координаты 0..1 видимой картинки ──
function contentBox(v: HTMLVideoElement) {
  const r = v.getBoundingClientRect(), vw = v.videoWidth || 16, vh = v.videoHeight || 9, k = Math.min(r.width / vw, r.height / vh);
  const w = vw * k, h = vh * k;
  return { x: r.left + (r.width - w) / 2, y: r.top + (r.height - h) / 2, w, h };
}
function norm(v: HTMLVideoElement, e: PointerEvent) {
  const b = contentBox(v);
  return { x: (e.clientX - b.x) / b.w, y: (e.clientY - b.y) / b.h, inside: e.clientX >= b.x && e.clientX <= b.x + b.w && e.clientY >= b.y && e.clientY <= b.y + b.h };
}
/** Навесить на холст поверх видео указку, рисование и управление. Действие Svelte: use:decorate={s}. */
export function decorate(c: HTMLCanvasElement, s0: any) {
  let s = s0;
  const v = c.parentElement!.querySelector('video') as HTMLVideoElement;
  c.tabIndex = 0;
  let line: any = null, lastSend = 0;
  const mode = () => SX.mode[s.id] || '';
  const on = (t: string, f: any, o?: any) => c.addEventListener(t, f, o);
  on('contextmenu', (e: Event) => { if (mode()) e.preventDefault(); });
  on('pointerdown', (e: PointerEvent) => {
    const m = mode(); if (!m) return;
    e.preventDefault(); c.focus({ preventScroll: true }); c.setPointerCapture(e.pointerId);
    const p = norm(v, e);
    if (m === 'ping') { if (p.inside) ann(s, { k: 'ping', x: +p.x.toFixed(4), y: +p.y.toFixed(4) }); return; }
    if (m === 'draw') { line = { k: 'line', id: SESSION + Date.now().toString(36), pts: [+p.x.toFixed(4), +p.y.toFixed(4)] }; return; }
    if (m === 'ctl') ctlPush({ t: 'd', b: e.button, x: +p.x.toFixed(5), y: +p.y.toFixed(5) });
  });
  on('pointermove', (e: PointerEvent) => {
    const m = mode(); if (!m) return;
    const p = norm(v, e);
    if (m === 'draw' && line) { line.pts.push(+p.x.toFixed(4), +p.y.toFixed(4)); localItem(s.id, ME(), line); if (Date.now() - lastSend > 60) { lastSend = Date.now(); ann(s, line); } return; }
    if (m === 'ctl' && p.inside) ctlPush({ t: 'm', x: +p.x.toFixed(5), y: +p.y.toFixed(5) });
  });
  const end = (e: PointerEvent) => {
    const m = mode();
    if (m === 'draw' && line) { line.done = true; ann(s, line); line = null; }
    if (m === 'ctl' && e.type === 'pointerup') { const p = norm(v, e); ctlPush({ t: 'u', b: e.button, x: +p.x.toFixed(5), y: +p.y.toFixed(5) }); }
  };
  on('pointerup', end); on('pointercancel', end);
  on('wheel', (e: WheelEvent) => { if (mode() !== 'ctl') return; e.preventDefault(); ctlPush({ t: 'w', dy: Math.round(e.deltaY), dx: Math.round(e.deltaX) }); }, { passive: false });
  on('keydown', (e: KeyboardEvent) => { if (mode() !== 'ctl' || !SX.cv.keyboard) return; e.preventDefault(); e.stopPropagation(); ctlPush({ t: 'kd', c: e.code }); });
  on('keyup', (e: KeyboardEvent) => { if (mode() !== 'ctl' || !SX.cv.keyboard) return; e.preventDefault(); e.stopPropagation(); ctlPush({ t: 'ku', c: e.code }); });
  const ro = new ResizeObserver(() => kick());
  ro.observe(v);
  return { update(n: any) { s = n; }, destroy() { ro.disconnect(); } };
}

// ── пометки ──
function localItem(stream: string, by: string, a: any) {
  const list = (AN.items[stream] = AN.items[stream] || []);
  if (a.k === 'clear') { AN.items[stream] = list.filter(x => x.by !== by); kick(); return; }
  if (a.k === 'line') { const i = list.findIndex(x => x.k === 'line' && x.id === a.id); const it = { ...a, by, t: Date.now(), col: colOf(by) }; if (i >= 0) list[i] = it; else list.push(it); }
  else list.push({ ...a, by, t: Date.now(), col: colOf(by) });
  if (list.length > 300) list.splice(0, list.length - 300);
  kick();
}
function ann(s: any, a: any) {
  localItem(s.id, ME(), a);
  const msg = { type: 'ann', stream: s.id, a };
  for (const n of R.members) if (n !== ME() && reachable(n)) rsend(n, msg).catch(() => {});
}
/** У показывающего — пометки на его экране (через мост и окно-наложение). */
function toScreen(stream: string, by: string, a: any) {
  const S = R.mine[stream];
  if (!S || S.surf !== 'monitor') return;
  api('POST', 'overlay/ann', { by, col: colOf(by), name: titleOf(by), a, w: S.w, h: S.h }).catch(() => {});
}
/** Пометка на снимке показа (w × h — размер кадра). */
function drawItem(ctx: CanvasRenderingContext2D, it: any, w: number, h: number) {
  ctx.strokeStyle = it.col; ctx.fillStyle = it.col; ctx.lineWidth = Math.max(3, w / 400); ctx.lineCap = 'round'; ctx.lineJoin = 'round';
  if (it.k === 'ping') { ctx.beginPath(); ctx.arc(it.x * w, it.y * h, Math.max(14, w / 80), 0, Math.PI * 2); ctx.stroke(); ctx.beginPath(); ctx.arc(it.x * w, it.y * h, 5, 0, Math.PI * 2); ctx.fill(); }
  else if (it.k === 'line' && it.pts.length >= 4) { ctx.beginPath(); ctx.moveTo(it.pts[0] * w, it.pts[1] * h); for (let i = 2; i < it.pts.length; i += 2) ctx.lineTo(it.pts[i] * w, it.pts[i + 1] * h); ctx.stroke(); }
}
function kick() { if (!AN.raf) AN.raf = requestAnimationFrame(draw); }
function draw() {
  AN.raf = 0;
  let alive = false;
  const now = Date.now();
  document.querySelectorAll<HTMLElement>('.streams [data-stream]').forEach(el => {
    const c = el.querySelector('canvas.ann') as HTMLCanvasElement | null, v = el.querySelector('video') as HTMLVideoElement | null;
    if (!c || !v) return;
    const r = c.getBoundingClientRect(), d = devicePixelRatio || 1;
    if (c.width !== Math.round(r.width * d) || c.height !== Math.round(r.height * d)) { c.width = Math.round(r.width * d); c.height = Math.round(r.height * d); }
    const ctx = c.getContext('2d')!;
    ctx.setTransform(d, 0, 0, d, 0, 0); ctx.clearRect(0, 0, r.width, r.height);
    const b = contentBox(v), ox = b.x - r.left, oy = b.y - r.top;
    const list = (AN.items[el.dataset.stream!] || []).filter(x => (x.k === 'ping' ? now - x.t < 2600 : !x.done || now - x.t < 6000));
    AN.items[el.dataset.stream!] = list;
    for (const it of list) {
      if (it.k === 'ping') {
        const age = (now - it.t) / 2600, px = ox + it.x * b.w, py = oy + it.y * b.h;
        for (let i = 0; i < 2; i++) { const a = Math.max(0, age - i * 0.18); ctx.strokeStyle = it.col; ctx.globalAlpha = Math.max(0, 1 - a) * 0.9; ctx.lineWidth = 3; ctx.beginPath(); ctx.arc(px, py, 6 + a * 46, 0, Math.PI * 2); ctx.stroke(); }
        ctx.globalAlpha = Math.max(0, 1 - age); ctx.fillStyle = it.col; ctx.beginPath(); ctx.arc(px, py, 5, 0, Math.PI * 2); ctx.fill();
        ctx.font = '700 12px Manrope, sans-serif';
        const label = it.by === ME() ? 'вы' : titleOf(it.by), tw = ctx.measureText(label).width;
        ctx.fillRect(px + 10, py - 26, tw + 10, 18); ctx.fillStyle = '#0B0C1A'; ctx.fillText(label, px + 15, py - 13);
      } else if (it.k === 'line' && it.pts.length >= 2) {
        const fade = it.done ? Math.max(0, 1 - Math.max(0, now - it.t - 4000) / 2000) : 1;
        ctx.globalAlpha = fade; ctx.strokeStyle = it.col; ctx.lineWidth = 4; ctx.lineCap = 'round'; ctx.lineJoin = 'round'; ctx.shadowColor = 'rgba(0,0,0,.6)'; ctx.shadowBlur = 4; ctx.beginPath();
        ctx.moveTo(ox + it.pts[0] * b.w, oy + it.pts[1] * b.h);
        for (let i = 2; i < it.pts.length; i += 2) ctx.lineTo(ox + it.pts[i] * b.w, oy + it.pts[i + 1] * b.h);
        ctx.stroke(); ctx.shadowBlur = 0;
      }
      alive = true;
    }
    ctx.globalAlpha = 1;
  });
  if (alive) AN.raf = requestAnimationFrame(draw);
}

// ── я управляю ──
function ctlPush(ev: any) {
  if (SX.cv.stream === '') return;
  if (ev.t === 'm') { const i = CVQ.q.findIndex(x => x.t === 'm'); if (i >= 0 && i === CVQ.q.length - 1) CVQ.q[i] = ev; else CVQ.q.push(ev); } else CVQ.q.push(ev);
  ctlFlush();
}
async function ctlFlush() {
  if (CVQ.busy || !CVQ.q.length) return;
  CVQ.busy = true;
  const ev = CVQ.q.splice(0, 200);
  try { await rsend(SX.cv.peer, { type: 'ctl', stream: SX.cv.stream, ev }); } catch {}
  CVQ.busy = false;
  if (CVQ.q.length) setTimeout(ctlFlush, 16);
}
export function release(notify: boolean) {
  if (!SX.cv.stream) return;
  const st = SX.cv.stream, peer = SX.cv.peer;
  SX.cv = { stream: '', peer: '', keyboard: false }; CVQ.q = [];
  if (SX.mode[st] === 'ctl') delete SX.mode[st];
  if (notify) rsend(peer, { type: 'ctl-end' }).catch(() => {});
}
export async function askControl(id: string, n: string, keyboard: boolean) {
  try { await rsend(n, { type: 'ctl-ask', stream: id, keyboard }); toast('Запрос отправлен — ждём ответа ' + titleOf(n)); } catch (e: any) { toast(e.message, 'bad'); }
}

// ── мной управляют ──
let keepT: any = 0;
export async function grantNow(peer: string, keyboard: boolean, stream: string) {
  const S = R.mine[stream];
  if (!S) { toast('Показ уже закончился', 'bad'); return; }
  try { await api('POST', 'control/grant', { peer, keyboard, w: S.w, h: S.h }); }
  catch (e: any) { toast('Управление: ' + e.message, 'bad'); rsend(peer, { type: 'ctl-no', stream, why: e.message }).catch(() => {}); return; }
  SX.ch = { peer, keyboard, stream, since: Date.now() };
  rsend(peer, { type: 'ctl-ok', stream, keyboard }).catch(() => {});
  clearInterval(keepT);
  keepT = setInterval(() => {
    if (!SX.ch) return clearInterval(keepT);
    if (!R.mine[SX.ch.stream] || !R.key) { revoke('показ закончился'); return; }
    api('POST', 'control/grant', { peer: SX.ch.peer, keyboard: SX.ch.keyboard, w: R.mine[SX.ch.stream].w, h: R.mine[SX.ch.stream].h }).catch(() => {});
  }, 8000);
}
export function revoke(why: string, quiet = false) {
  if (!SX.ch) return;
  const p = SX.ch.peer;
  SX.ch = null; clearInterval(keepT);
  api('POST', 'control/revoke', { why: why || '' }).catch(() => {});
  if (!quiet) rsend(p, { type: 'ctl-end', why: why || '' }).catch(() => {});
}
let askT: any = 0;
function askDialog(n: string, m: any) {
  SX.ask = { n, stream: m.stream, keyboard: !!m.keyboard };
  ring(true);
  clearTimeout(askT);
  askT = setTimeout(() => { if (SX.ask && SX.ask.n === n) { askDecline('нет ответа'); } }, 60000);
}
export function askDecline(why = '') {
  const a = SX.ask; if (!a) return;
  SX.ask = null; ring(false); clearTimeout(askT);
  rsend(a.n, { type: 'ctl-no', stream: a.stream, why: why || undefined }).catch(() => {});
}
export function askGrant(keyboard: boolean) {
  const a = SX.ask; if (!a) return;
  SX.ask = null; ring(false); clearTimeout(askT);
  grantNow(a.n, keyboard, a.stream);
}

// ── сообщения комнаты ──
onRoomMsg((m, n, k) => {
  if (k !== R.key) return false;
  switch (m.type) {
    case 'ann': { if (!m.a) return true; if (R.watching.has(m.stream)) localItem(m.stream, n, m.a); if (R.mine[m.stream]) toScreen(m.stream, n, m.a); return true; }
    case 'ctl-ask': askDialog(n, m); return true;
    case 'ctl-ok':
      if (SX.cv.peer === n || !SX.cv.stream) {
        SX.cv = { stream: m.stream, peer: n, keyboard: !!m.keyboard }; CVQ.q = []; SX.mode[m.stream] = 'ctl';
        toast(titleOf(n) + ' разрешил управление: ' + (m.keyboard ? 'мышь и клавиатура' : 'мышь') + '. Щёлкните по его экрану', 'ok');
      }
      return true;
    case 'ctl-no': toast(titleOf(n) + ' не разрешил управление' + (m.why ? ' (' + m.why + ')' : ''), 'bad'); return true;
    case 'ctl-end':
      if (SX.cv.peer === n && SX.cv.stream) { release(false); toast('Управление ПК ' + titleOf(n) + ' закончилось' + (m.why ? ': ' + m.why : '')); }
      if (SX.ch && SX.ch.peer === n) revoke('отпустил', true);
      return true;
    case 'ctl':
      if (SX.ch && SX.ch.peer === n && Array.isArray(m.ev)) api('POST', 'control/input', { peer: n, events: m.ev }).catch(e => { if (/не разрешено|другому/.test(e.message)) revoke(e.message); });
      return true;
  }
  return false;
});
// трей или мост остановил управление (в том числе из-за защиты)
onUiMsg(m => {
  if (m.type !== 'ctl-revoked') return false;
  if (SX.ch) { const p = SX.ch.peer; SX.ch = null; clearInterval(keepT); rsend(p, { type: 'ctl-end', why: m.why || 'остановлено' }).catch(() => {}); }
  return true;
});
onLeave(quiet => { release(!quiet); revoke('вышел из канала'); });
onShareStop(id => { if (SX.ch && SX.ch.stream === id) revoke('показ закончился'); });
addEventListener('beforeunload', () => { if (SX.ch) try { navigator.sendBeacon(aurl('control/revoke'), new Blob(['{}'], { type: 'application/json' })); } catch {} });

// ── пульт ассистента: снимок показа и пометки на нём ──
// Управлять чужим ПК ассистент не может: только человек — запросом в панели и подтверждением у показывающего.
function apiStream(a: any) {
  if (!R.key) throw new Error('панель не в канале — сначала ui join {channel}');
  if (a.stream && R.mine[a.stream]) return { id: a.stream, n: ME(), mine: true, label: R.mine[a.stream].label };
  if (a.mine) { const id = Object.keys(R.mine)[0]; if (!id) throw new Error('вы ничего не показываете'); return { id, n: ME(), mine: true, label: R.mine[id].label }; }
  for (const p of rosterOf(R.key)) for (const x of p.st.streams || []) if (x.id === a.stream || (!a.stream && a.peer && p.name.toLowerCase() === String(a.peer).toLowerCase())) return { id: x.id, n: p.name, mine: false, label: x.label };
  throw new Error(a.stream ? 'нет такого показа' : a.peer ? titleOf(a.peer) + ' ничего не показывает' : 'нужен peer или stream');
}
const wait = async (f: () => any, ms: number) => { const t0 = Date.now(); while (Date.now() - t0 < ms) { const v = f(); if (v) return v; await new Promise(r => setTimeout(r, 150)); } return f(); };
addUi('screen_shot', async a => {
  const st = apiStream(a);
  let started = false;
  let ms: MediaStream | null = st.mine ? R.mine[st.id].stream || null : R.remote[st.id] || null;
  if (!st.mine && !ms) { watch(st.id, true); started = true; ms = await wait(() => R.remote[st.id], 12000); }
  if (!ms) throw new Error('видео показа не пришло');
  try {
    const v = document.createElement('video'); v.muted = true; v.playsInline = true; v.srcObject = ms;
    await v.play().catch(() => {});
    if (!(await wait(() => v.videoWidth > 0 && v.readyState >= 2, 8000))) throw new Error('кадр не пришёл');
    await new Promise(r => setTimeout(r, 250));
    const max = Math.max(320, Math.min(3840, +a.maxSize || 1920)), k = Math.min(1, max / Math.max(v.videoWidth, v.videoHeight));
    const c = document.createElement('canvas'); c.width = Math.round(v.videoWidth * k); c.height = Math.round(v.videoHeight * k);
    const ctx = c.getContext('2d')!; ctx.drawImage(v, 0, 0, c.width, c.height);
    if (a.marks !== false) { for (const it of AN.items[st.id] || []) drawItem(ctx, it, c.width, c.height); }
    v.srcObject = null;
    const blob: Blob = await new Promise(r => c.toBlob(b => r(b!), a.format === 'jpeg' ? 'image/jpeg' : 'image/png', 0.9));
    const r = await api('POST', 'shots/save?kind=screen', blob, true);
    return { path: r.path, width: c.width, height: c.height, peer: st.n, stream: st.id, label: st.label };
  } finally { if (started && a.keep !== true) watch(st.id, false); }
});
addUi('screen_point', async a => {
  const st = apiStream(a);
  const x = +a.x, y = +a.y;
  if (!(x >= 0 && x <= 1 && y >= 0 && y <= 1)) throw new Error('x, y — доли экрана от 0 до 1 (0,0 — левый верх)');
  ann({ id: st.id }, { k: 'ping', x: +x.toFixed(4), y: +y.toFixed(4) });
  return { stream: st.id, peer: st.n };
});
addUi('screen_draw', async a => {
  const st = apiStream(a);
  const pts: number[] = [];
  for (const p of a.points || []) { const x = Array.isArray(p) ? +p[0] : +p.x, y = Array.isArray(p) ? +p[1] : +p.y; if (x >= 0 && x <= 1 && y >= 0 && y <= 1) pts.push(+x.toFixed(4), +y.toFixed(4)); }
  if (pts.length < 4) throw new Error('points: хотя бы две точки [[x, y], …], доли экрана 0…1');
  ann({ id: st.id }, { k: 'line', id: SESSION + Date.now().toString(36), pts: pts.slice(0, 4000), done: true });
  return { stream: st.id, peer: st.n, points: pts.length / 2 };
});
addUi('screen_clear', async a => { const st = apiStream(a); ann({ id: st.id }, { k: 'clear' }); return { stream: st.id }; });

// ── громкость звука показа: своя для каждого показывающего, запоминается ──
export function streamVol(id: string): number { const st = streamOf(id); const v = st ? (SET.svol || {})[st.n] : undefined; return typeof v === 'number' ? v : 1; }
export function setStreamVol(id: string, v: number) {
  const st = streamOf(id); if (!st) return;
  setSet('svol', { ...(SET.svol || {}), [st.n]: Math.max(0, Math.min(1, v)) });
  document.querySelectorAll<HTMLVideoElement>('.stream[data-stream="' + CSS.escape(id) + '"] video').forEach(x => (x.volume = Math.max(0, Math.min(1, v))));
  try { const w = R.popups[id]; if (w && !w.closed) (w as any).__setVol?.(v); } catch {}
}
// отдельное окно показа (popout.html) берёт пометки и инструменты у панели, которая его открыла
(window as any).__scr = {
  vol: (id: string) => streamVol(id),
  setVol: (id: string, v: number) => setStreamVol(id, v),
  ann: (id: string, a: any) => { if (R.remote[id]) ann({ id }, a); },
  items: (id: string) => AN.items[id] || [],
  me: () => ME(),
  title: (n: string) => titleOf(n),
  session: SESSION,
};

// Связь панели с мостом.
// В окне приложения (Tauri) панель открыта по протоколу bridge:// — запросы идут в мост внутри программы, без сети
// и без ключа. В браузере (запасной вариант) — на 127.0.0.1 с локальным ключом из страницы.

export const IS_APP = typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
export const TOKEN: string = (document.querySelector('meta[name=bridge-token]') as HTMLMetaElement | null)?.content?.replace('%%TOKEN%%', '') || '';
export const API = '/bridge/v1/local/';

export class ApiError extends Error {
  status: number;
  constructor(msg: string, status: number) { super(msg); this.status = status; }
}

/** Запрос к локальному API: JSON туда и обратно (raw — тело как есть: файл, Blob). */
export async function api<T = any>(method: string, path: string, body?: unknown, raw?: boolean): Promise<T> {
  const opt: RequestInit = { method, headers: authHeaders() };
  if (body !== undefined) {
    opt.body = raw ? (body as BodyInit) : JSON.stringify(body);
    if (!raw) (opt.headers as Record<string, string>)['Content-Type'] = 'application/json';
  }
  const r = await fetch(API + path, opt);
  const text = await r.text();
  let j: any = null;
  try { j = JSON.parse(text); } catch {}
  if (!r.ok) throw new ApiError((j && j.error) || text || String(r.status), r.status);
  return j as T;
}

export function authHeaders(): Record<string, string> {
  return TOKEN ? { 'X-Bridge-Local': TOKEN } : {};
}

/** Адрес для <img>, <audio>, ссылок: в браузере ключ — в запросе (картинке заголовок не передать). */
export function aurl(path: string): string {
  if (!TOKEN) return API + path;
  return API + path + (path.includes('?') ? '&' : '?') + 'token=' + encodeURIComponent(TOKEN);
}

/** Действие с записями пространства (actions). */
export async function act(a: Record<string, unknown>) {
  const r = await api('POST', 'actions', a);
  if (!r.ok) throw new Error(r.error);
  return r;
}

/** Выгрузка с ходом (XMLHttpRequest умеет onprogress на отправке). */
/** Больше этого — частями: окно приложения передаёт тело запроса в мост целиком, большие файлы так не проходят. */
const PART = 8 * 1024 * 1024;
function post(path: string, body: Blob | null, onProgress?: (e: ProgressEvent) => void): Promise<any> {
  return new Promise((resolve, reject) => {
    const xhr = new XMLHttpRequest();
    xhr.open('POST', API + path);
    for (const [k, v] of Object.entries(authHeaders())) xhr.setRequestHeader(k, v);
    if (onProgress) xhr.upload.onprogress = onProgress;
    xhr.onload = () => {
      let j: any = null;
      try { j = JSON.parse(xhr.responseText); } catch {}
      if (xhr.status === 200) resolve(j);
      else reject(new ApiError((j && j.error) || xhr.responseText || String(xhr.status), xhr.status));
    };
    xhr.onerror = () => reject(new ApiError('нет связи с мостом', 0));
    xhr.send(body);
  });
}
export async function upload(path: string, file: Blob, onProgress?: (part: number) => void): Promise<any> {
  if (file.size <= PART) return uploadOne(path, file, onProgress);
  const id = (crypto as any).randomUUID ? crypto.randomUUID() : Date.now().toString(36) + Math.random().toString(36).slice(2);
  for (let off = 0; off < file.size; off += PART) {
    const chunk = file.slice(off, Math.min(file.size, off + PART));
    let err: any = null;
    for (let attempt = 0; attempt < 3; attempt++) { // часть не дошла — ещё раз
      try { await post('upload/part?id=' + id + '&offset=' + off, chunk, e => { if (e.lengthComputable && onProgress) onProgress((off + e.loaded) / file.size); }); err = null; break; }
      catch (e: any) { err = e; if (e.status === 409 || e.status === 413) break; await new Promise(r => setTimeout(r, 500)); }
    }
    if (err) throw err;
  }
  const r = await post(path + (path.includes('?') ? '&' : '?') + 'upload=' + id + '&size=' + file.size, null);
  if (onProgress) onProgress(1);
  return r;
}
function uploadOne(path: string, file: Blob, onProgress?: (part: number) => void): Promise<any> {
  return new Promise((resolve, reject) => {
    const xhr = new XMLHttpRequest();
    xhr.open('POST', API + path);
    for (const [k, v] of Object.entries(authHeaders())) xhr.setRequestHeader(k, v);
    xhr.upload.onprogress = e => { if (e.lengthComputable && onProgress) onProgress(e.loaded / e.total); };
    xhr.onload = () => {
      let j: any = null;
      try { j = JSON.parse(xhr.responseText); } catch {}
      if (xhr.status === 200) resolve(j);
      else reject(new ApiError((j && j.error) || xhr.responseText || String(xhr.status), xhr.status));
    };
    xhr.onerror = () => reject(new ApiError('нет связи с мостом', 0));
    xhr.send(file);
  });
}

/** Скачать файл моста как файл (с ключом в заголовке). */
export async function download(path: string, name: string) {
  const r = await fetch(API + path, { headers: authHeaders() });
  if (!r.ok) throw new ApiError(await r.text(), r.status);
  const b = await r.blob();
  const u = URL.createObjectURL(b);
  const l = document.createElement('a');
  l.href = u; l.download = name; l.click();
  setTimeout(() => URL.revokeObjectURL(u), 5000);
}

// ── поток сигналов (звонки, канал, команды ассистента) ──

type MediaHandler = (msg: any) => void;
const handlers = new Set<MediaHandler>();
let started = false;
let es: EventSource | null = null;
let onReconnect: (() => void) | null = null;

/** Подписка на сигналы моста. В приложении — события окна, в браузере — EventSource. */
export function onMedia(h: MediaHandler): () => void {
  handlers.add(h);
  if (!started) { started = true; startMedia(); }
  return () => handlers.delete(h);
}

/** Вызывается, когда поток пересоздан (мост перезапустился) — панель перечитывает состояние. */
export function onMediaReconnect(f: () => void) { onReconnect = f; }

function dispatch(m: any) { for (const h of handlers) { try { h(m); } catch (e) { console.error(e); } } }

async function startMedia() {
  if (IS_APP) {
    const { listen } = await import('@tauri-apps/api/event');
    await listen<any>('bridge-media', e => dispatch(e.payload));
    await listen<any>('bridge-reconnect', () => onReconnect && onReconnect());
    return;
  }
  const open = () => {
    es = new EventSource(API + 'media/stream?token=' + encodeURIComponent(TOKEN));
    es.addEventListener('signal', (e: MessageEvent) => { try { dispatch(JSON.parse(e.data)); } catch {} });
    let wasOpen = false;
    es.onopen = () => { if (wasOpen && onReconnect) onReconnect(); wasOpen = true; };
  };
  open();
}

/** Команда окну приложения (свернуть в трей, заголовок и т. п.). В браузере — ничего. */
export async function app(cmd: string, body?: unknown) {
  if (!IS_APP) return null;
  try {
    const r = await fetch('/__app/' + cmd, { method: 'POST', body: body === undefined ? undefined : JSON.stringify(body) });
    return r.ok ? await r.json().catch(() => null) : null;
  } catch { return null; }
}

// Общие мелочи панели: размеры, время, экранирование, markdown журнала, сравнение версий.

export const esc = (s: unknown) => String(s ?? '').replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]!));

export const size = (n: number | null | undefined) =>
  n == null ? '' : n < 1024 ? n + ' Б' : n < 1048576 ? (n / 1024).toFixed(1) + ' КБ' : n < 1073741824 ? (n / 1048576).toFixed(1) + ' МБ' : (n / 1073741824).toFixed(2) + ' ГБ';

export function ago(t: string | number | null | undefined): string {
  if (!t) return '—';
  const s = (Date.now() - new Date(t).getTime()) / 1000;
  if (s < 5) return 'только что';
  if (s < 60) return Math.round(s) + ' с назад';
  if (s < 3600) return Math.round(s / 60) + ' мин назад';
  if (s < 86400) return Math.round(s / 3600) + ' ч назад';
  return new Date(t).toLocaleString('ru-RU');
}

export const fullTime = (t: string | number | null | undefined) => (t ? new Date(t).toLocaleString('ru-RU') : '');

export const dur = (s: number) =>
  s < 0 ? '' : s < 60 ? s + ' с' : s < 3600 ? Math.floor(s / 60) + ' мин ' + (s % 60) + ' с' : Math.floor(s / 3600) + ' ч ' + Math.floor((s % 3600) / 60) + ' мин';

/** Простой markdown журнала изменений: заголовки, списки, **жирный**, `код`. */
export function md(text: string): string {
  const inl = (x: string) => esc(x).replace(/\*\*(.+?)\*\*/g, '<b>$1</b>').replace(/`([^`]+)`/g, '<code>$1</code>');
  let html = '', inList = false;
  for (const raw of String(text || '').replace(/\r/g, '').split('\n')) {
    const l = raw.trimEnd();
    if (/^- /.test(l)) { if (!inList) { html += '<ul>'; inList = true; } html += '<li>' + inl(l.slice(2)) + '</li>'; continue; }
    if (inList) { html += '</ul>'; inList = false; }
    if (/^## /.test(l)) html += '<h3 class="cl-v">' + inl(l.slice(3)) + '</h3>';
    else if (/^# /.test(l)) continue;
    else if (l.trim()) html += '<p>' + inl(l) + '</p>';
  }
  return html + (inList ? '</ul>' : '');
}

export function vcmp(a: string, b: string): number {
  const p = (x: string) => String(x || '').split('.').map(n => parseInt(n) || 0);
  const x = p(a), y = p(b);
  for (let i = 0; i < 3; i++) if ((x[i] || 0) !== (y[i] || 0)) return (x[i] || 0) - (y[i] || 0);
  return 0;
}

export const STATUS: Record<string, [string, string]> = {
  new: ['новая', 'acc'], accepted: ['принята', 'acc'], in_progress: ['в работе', 'warn'], review: ['на проверке', 'warn'], done: ['готово', 'ok'],
  rejected: ['отказ', 'bad'], cancelled: ['отменена', ''], pending: ['ждёт', 'warn'], running: ['выполняется', 'warn'], failed: ['ошибка', 'bad'], proposed: ['предложено', 'acc'],
};
export const statusOf = (s: string) => STATUS[s] || [s, ''];

export function copy(t: string, toast: (m: string, k?: string) => void) {
  navigator.clipboard?.writeText(t).then(() => toast('Скопировано'), () => toast('Не удалось скопировать', 'bad'));
}

/** Файлы из перетаскивания, с папками целиком (путь — от перетащенной папки). */
export async function dropFiles(e: DragEvent): Promise<{ file: File; path: string }[]> {
  const dt = e.dataTransfer;
  if (!dt) return [];
  const items = [...dt.items].map(i => (i as any).webkitGetAsEntry && (i as any).webkitGetAsEntry()).filter(Boolean);
  const out: { file: File; path: string }[] = [];
  async function walk(entry: any, prefix: string) {
    if (entry.isFile) await new Promise<void>(r => entry.file((f: File) => { out.push({ file: f, path: prefix + f.name }); r(); }, () => r()));
    else if (entry.isDirectory) {
      const rd = entry.createReader();
      let batch: any[];
      do {
        batch = await new Promise<any[]>(r => rd.readEntries(r, () => r([])));
        for (const c of batch) await walk(c, prefix + entry.name + '/');
      } while (batch.length);
    }
  }
  if (items.length) { for (const it of items) await walk(it, ''); return out; }
  return [...dt.files].map(f => ({ file: f, path: f.name }));
}

export function pickFiles(opts: { multiple?: boolean; accept?: string; dir?: boolean } = {}): Promise<File[]> {
  return new Promise(resolve => {
    const i = document.createElement('input');
    i.type = 'file';
    if (opts.multiple) i.multiple = true;
    if (opts.accept) i.accept = opts.accept;
    if (opts.dir) (i as any).webkitdirectory = true;
    i.onchange = () => resolve([...(i.files || [])]);
    i.click();
  });
}

const TR: Record<string, string> = { а: 'a', б: 'b', в: 'v', г: 'g', д: 'd', е: 'e', ё: 'e', ж: 'zh', з: 'z', и: 'i', й: 'y', к: 'k', л: 'l', м: 'm', н: 'n', о: 'o', п: 'p', р: 'r', с: 's', т: 't', у: 'u', ф: 'f', х: 'h', ц: 'c', ч: 'ch', ш: 'sh', щ: 'sch', ы: 'y', э: 'e', ю: 'yu', я: 'ya' };
/** Имя → id латиницей: «Общий канал» → obschiy-kanal. */
export function translit(s: string): string {
  return s.toLowerCase().split('').map(ch => (TR[ch] !== undefined ? TR[ch] : /[a-z0-9_-]/.test(ch) ? ch : ch === ' ' ? '-' : '')).join('').replace(/-+/g, '-').replace(/^-|-$/g, '').slice(0, 40);
}

export const clamp =(v: number, a: number, b: number) => Math.max(a, Math.min(b, v));
export const sleep = (ms: number) => new Promise(r => setTimeout(r, ms));

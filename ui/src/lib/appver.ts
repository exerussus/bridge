// (3.23) Сведения о сборке программы из version.json (его пишет AppCore при сборке Unity). Файла может не быть.
import { toast } from './state.svelte';

export type AppInfo = {
  version: string; fullVersion?: string; hash?: string; dirty?: boolean;
  profile?: string; platform?: string; development?: boolean; builtAtUtc?: string;
};

/** Время сборки — по часам этого ПК: «8 нояб., 21:19». */
export function builtLocal(a: AppInfo | null | undefined): string {
  if (!a?.builtAtUtc) return '';
  const d = new Date(a.builtAtUtc);
  return isNaN(+d) ? '' : d.toLocaleString('ru-RU', { day: 'numeric', month: 'short', hour: '2-digit', minute: '2-digit' });
}

/** Подсказка со всеми полями — для title. */
export function appTitle(a: AppInfo | null | undefined): string {
  if (!a) return '';
  return [
    'Версия ' + (a.fullVersion || a.version),
    a.hash ? 'коммит ' + a.hash + (a.dirty ? ' (с незакоммиченными правками)' : '') : '',
    [a.profile, a.platform].filter(Boolean).join(' · '),
    a.development ? 'Development build' : a.development === false ? 'Release' : '',
    builtLocal(a) ? 'собрана ' + builtLocal(a) : '',
  ].filter(Boolean).join('\n');
}

export async function copyHash(a: AppInfo | null | undefined) {
  const h = a?.hash || '';
  if (!h) return;
  try { await navigator.clipboard.writeText(h); toast('Хэш коммита скопирован', 'ok'); } catch { toast(h, 'ok'); }
}

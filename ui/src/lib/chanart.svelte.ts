// Оформление канала: иконка и подложка (картинка с маской, прозрачностью, оттенком — как у плитки).
// Хранит владелец канала; мост забирает по отпечатку `art` из описания канала, здесь — кэш.
import { api, aurl } from './api';
import type { TileStyle } from './tile.svelte';

type Art = { icon: string; back: string; style: TileStyle; h: string };
export const CART = $state<Record<string, Art>>({});
const loading = new Set<string>();

/** Оформление канала (реактивно). c — канал из списка: {owner, id, art}. */
export function artOf(c: { owner: string; id: string; art?: string } | null | undefined): Art | null {
  if (!c || !c.art) return null;
  const k = c.owner + ':' + c.id, t = CART[k];
  if ((!t || t.h !== c.art) && !loading.has(k + c.art)) {
    loading.add(k + c.art);
    api('GET', 'chan/art?room=' + encodeURIComponent(k)).then(r => { CART[k] = { icon: r.icon || '', back: r.back || '', style: r.style || {}, h: c.art! }; }).catch(() => {}).finally(() => loading.delete(k + c.art));
  }
  return t || null;
}
export const artImg = (c: { owner: string; id: string }, what: 'icon' | 'back', hash: string) =>
  hash ? aurl('chan/img?room=' + encodeURIComponent(c.owner + ':' + c.id) + '&what=' + what + '&v=' + hash) : '';

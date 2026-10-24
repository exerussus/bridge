import { api } from './api';
import { load, toast } from './state.svelte';

export async function removePeer(p: any): Promise<boolean> {
  if (!confirm('Удалить соседа «' + (p.title || p.name) + '»?\n\nСвязь с ним прекратится: он пропадёт из пространств, доступа к общим папкам и правил приёма.\nФайлы (принятые, неотправленные, копии его папок) и история задач останутся.\n\nЕсли нужно лишь на время — «⏸ Выключить» вместо удаления.')) return false;
  try { await api('POST', 'peers/remove?name=' + encodeURIComponent(p.name)); toast('Сосед удалён', 'ok'); await load(); return true; } catch (e: any) { toast(e.message, 'bad'); return false; }
}

export const STEP: Record<string, [string, string]> = { ok: ['✓', 'ok'], warn: ['!', 'warn'], bad: ['✗', 'bad'], info: ['i', ''] };

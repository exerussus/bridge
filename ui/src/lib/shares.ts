import { api } from './api';
import { closeModal, load, toast } from './state.svelte';

export async function shareDel(id: string) {
  if (!confirm('Убрать пространство «' + id + '» с этого ПК?\nПапка и файлы останутся на диске; записи и обмен с соседями по нему прекратятся. Подключить заново можно тем же id.')) return;
  try { await api('POST', 'shares/remove?id=' + encodeURIComponent(id)); closeModal(); toast('Пространство убрано — папка осталась на диске', 'ok'); await load(); } catch (e: any) { toast(e.message, 'bad'); }
}

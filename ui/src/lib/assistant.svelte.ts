// Ассистент: история живёт в мосте (bridge-data/assistant/history.jsonl). Панель держит долгий опрос
// assistant/chat: новые сообщения, «печатает…», на связи ли ассистент.
import { SvelteMap } from 'svelte/reactivity';
import { api } from './api';
import { ui, toast, badgeCount } from './state.svelte';
import { notifyTo } from './notify';
import { msgBeep, alarmBeep } from './audio';

export const AS = $state({ n: 0, host: null as string | null, title: 'Ассистент', online: false, typing: false, unread: 0, isHost: false, alarm: false, first: true, pending: 0, stick: false });
export const AS_ITEMS = new SvelteMap<string, any>();

let alarmT: any = 0;
export function alarmStart() {
  clearInterval(alarmT);
  AS.alarm = true;
  let i = 0;
  alarmT = setInterval(() => { if (!AS.alarm || ++i > 120) { alarmStop(); return; } alarmBeep(); }, 1500);
}
export function alarmStop() { AS.alarm = false; clearInterval(alarmT); }

function asNotify(it: any) {
  if (it.kind === 'notice') {
    if (it.alarm) alarmStart(); else msgBeep();
    if (document.hidden || ui.tab !== 'assistant') notifyTo(AS.title, it.text || 'новое сообщение', 'assistant');
    if (ui.tab !== 'assistant') toast(AS.title + ': ' + (it.text || '').slice(0, 120));
  } else if (ui.tab !== 'assistant' || document.hidden) msgBeep();
}

export function asRead() { if (AS.unread && !document.hidden) api('POST', 'assistant/read').catch(() => {}); }

let started = false;
export async function asStart() {
  if (started) return;
  started = true;
  for (;;) {
    try {
      const r = await api('GET', 'assistant/chat?after=' + AS.n + '&timeout=' + (AS.first ? 0 : 25));
      const fresh: any[] = [];
      for (const it of r.items) { if (!AS_ITEMS.has(it.id) && it.dir === 'in' && !AS.first) fresh.push(it); AS_ITEMS.set(it.id, it); AS.n = Math.max(AS.n, it.n); }
      Object.assign(AS, { host: r.host, title: r.title, online: r.online, typing: r.typing, unread: r.unread, isHost: r.isHost, pending: r.pending });
      AS.first = false;
      for (const it of fresh) asNotify(it);
      badgeCount('assistant', ui.tab === 'assistant' ? 0 : AS.unread);
      if (ui.tab === 'assistant') asRead();
    } catch { await new Promise(r => setTimeout(r, 5000)); }
  }
}

// (3.21) Свой сервер и своя сеть — коротко для боковой панели и «Обзора»: опрос раз в 15 с, пока панель видна.
import { api } from './api';

export const NS = $state({ relay: null as any, net: null as any, at: 0 });

let started = false;
async function poll() {
  if (document.hidden && NS.at) return;
  try { const r = await api('GET', 'relay'); NS.relay = { ...(r.status || {}), link: r.link !== false, linkUp: !!r.linkUp, files: !!r.files, net: !!r.net, netUp: !!r.netUp }; } catch {}
  try { NS.net = await api('GET', 'mostnet'); } catch {}
  NS.at = Date.now();
}
export function netStart() {
  if (started) return;
  started = true;
  poll();
  setInterval(poll, 15000);
}
export const netPoll = poll;

/** Сервер: точка и строка. */
export function relayLine(): { dot: string; text: string; sub: string } {
  const s = NS.relay;
  if (!s || !s.configured) return { dot: '', text: 'Свой сервер', sub: 'не подключён' };
  if (s.ok) return { dot: 'ok', text: s.name || s.host || 'Свой сервер', sub: (s.rtt != null ? s.rtt + ' мс' : 'на связи') };
  return { dot: 'bad', text: s.name || s.host || 'Свой сервер', sub: 'нет связи' };
}
/** Своя сеть: точка и строка. */
export function netLine(): { dot: string; text: string; sub: string } {
  const d = NS.net, svc = d?.service;
  if (!d || !d.relay || !svc) return { dot: '', text: 'Своя сеть', sub: 'не установлена' };
  if (!d.enabled || svc.state === 'off') return { dot: '', text: 'Своя сеть', sub: 'выключена' };
  if (svc.state === 'on') return { dot: 'ok', text: 'Своя сеть', sub: svc.vip || 'включена' };
  if (svc.state === 'connecting') return { dot: 'warn', text: 'Своя сеть', sub: 'подключаюсь…' };
  return { dot: 'bad', text: 'Своя сеть', sub: 'ошибка' };
}

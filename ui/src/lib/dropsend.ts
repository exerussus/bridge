// Быстрая отправка: файлы (и папки), брошенные на плитку участника в канале, уходят ему ручной отправкой
// (у него — в «приём/<этот ПК>»). Ход видно в плашке передач в углу панели.
import { upload } from './api';
import { ui, toast, titleOf } from './state.svelte';
import { dropFiles, size } from './util';

const hasFiles = (e: DragEvent) => !!e.dataTransfer && [...e.dataTransfer.types].includes('Files');
/** Можно ли отправить: только соседу этого ПК (через чужой мост ручная отправка не идёт). */
const peerOf = (n: string) => ((ui.ov && ui.ov.peers) || []).find((p: any) => String(p.name).toLowerCase() === String(n).toLowerCase() && p.enabled && !p.invite && !p.pending);
export const canSendTo = (n: string) => !!peerOf(n);

export async function sendFiles(peer: string, list: { file: File; path: string }[]) {
  if (!list.length) return;
  const total = list.reduce((a, x) => a + x.file.size, 0);
  toast('Отправляю ' + titleOf(peer) + ': ' + (list.length === 1 ? list[0].path : list.length + ' файлов') + ' · ' + size(total));
  let ok = 0; const bad: string[] = [];
  for (const it of list) {
    try { await upload('mailbox/upload?' + new URLSearchParams({ peer, path: it.path }), it.file); ok++; }
    catch (e: any) { bad.push(it.path + ': ' + e.message); }
  }
  if (bad.length) toast('Не ушло ' + bad.length + ' из ' + list.length + ': ' + bad.slice(0, 2).join('; '), 'bad');
  else toast('Отправлено ' + titleOf(peer) + ': ' + (ok === 1 ? list[0].path : ok + ' файлов') + ' — у него в «приём»', 'ok');
}

/** Действие Svelte: use:dropSend={name}. Своя плитка и не соседи — отказ с понятной подсказкой. */
export function dropSend(node: HTMLElement, name: string | null) {
  let n = name, depth = 0;
  const off = () => { depth = 0; node.classList.remove('dropon', 'dropno'); };
  const enter = (e: DragEvent) => {
    if (!n || !hasFiles(e)) return;
    e.preventDefault(); depth++;
    const ok = canSendTo(n);
    node.classList.toggle('dropon', ok); node.classList.toggle('dropno', !ok);
    node.dataset.drop = ok ? 'Отправить ' + titleOf(n) : titleOf(n) + ' — не сосед этого ПК';
  };
  const over = (e: DragEvent) => { if (!n || !hasFiles(e)) return; e.preventDefault(); if (e.dataTransfer) e.dataTransfer.dropEffect = canSendTo(n) ? 'copy' : 'none'; };
  const leave = () => { if (--depth <= 0) off(); };
  const drop = async (e: DragEvent) => {
    if (!n || !hasFiles(e)) return;
    e.preventDefault(); e.stopPropagation(); off();
    if (!canSendTo(n)) { toast(titleOf(n) + ' — не сосед этого ПК: отправить можно только соседу', 'bad'); return; }
    sendFiles(peerOf(n).name, await dropFiles(e));
  };
  node.addEventListener('dragenter', enter); node.addEventListener('dragover', over); node.addEventListener('dragleave', leave); node.addEventListener('drop', drop);
  return {
    update(v: string | null) { n = v; },
    destroy() { node.removeEventListener('dragenter', enter); node.removeEventListener('dragover', over); node.removeEventListener('dragleave', leave); node.removeEventListener('drop', drop); },
  };
}

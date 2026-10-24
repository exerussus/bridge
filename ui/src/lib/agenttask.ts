// Задача → агент: поручить запись-задачу Claude на соседнем ПК. Исполнитель задачи — ПК агента: тогда его мост сам
// ведёт статус (принята → в работе → на проверке), «готово» ставит заказчик.
import { api, act } from './api';
import { ui, setTab, toast, titleOf } from './state.svelte';

/** Кому этот ПК может поручать: сам себе (агент здесь включён) и соседи, которые разрешают. */
export async function agentPeers(): Promise<{ name: string; mode: string; local?: boolean }[]> {
  try {
    const r = await api('GET', 'agent');
    const l = r.local && r.local.mode !== 'off' ? [{ name: r.local.name, mode: r.local.mode, local: true }] : [];
    return l.concat((r.peers || []).filter((p: any) => p.agent).map((p: any) => ({ name: p.name, mode: p.agent })));
  } catch { return []; }
}

/** Текст поручения из задачи. */
export function taskPrompt(share: string, rec: any): string {
  const f = rec.fields || {};
  const extra = Object.entries(f).filter(([k]) => !['title', 'body', 'assignee', 'agent', 'agentThread', 'deleted'].includes(k)).map(([k, v]) => '- ' + k + ': ' + (typeof v === 'string' ? v : JSON.stringify(v))).join('\n');
  return 'Задача «' + (f.title || rec.id) + '» (запись ' + rec.id + ' в пространстве «' + share + '»).\n\n' + (f.body ? f.body + '\n\n' : '') + (extra ? 'Поля задачи:\n' + extra + '\n\n' : '') +
    'Сделай задачу. Итог — коротко: что сделано, где результат, что осталось.';
}

/** Поручить: назначить исполнителем ПК агента (если можно), запустить поручение, отметить в записи. */
export async function delegate(share: string, rec: any, peer: string, prompt: string, assign: boolean): Promise<string> {
  const me = ui.ov.node.node;
  if (assign && rec.fields?.assignee !== peer && rec.creator === me) {
    try { await act({ action: 'set', share, record: rec.id, fields: { assignee: peer }, actor: 'панель' }); } catch (e: any) { toast('Исполнителя сменить не удалось: ' + e.message, 'bad'); }
  }
  const local = peer === me;
  const t = await api('POST', 'agent/ask', { peer: local ? '' : peer, prompt, title: rec.fields?.title || rec.id, record: { share, id: rec.id } });
  try { await act({ action: 'set', share, record: rec.id, fields: { agent: peer, agentThread: t.id }, actor: 'панель' }); } catch {}
  toast('Поручено агенту ' + (local ? 'на этом ПК' : 'на ' + titleOf(peer)) + ' — статус задачи он будет вести сам', 'ok');
  return t.id;
}

/** Открыть нить во вкладке «Агенты». */
export function openThread(id: string) { ui.agentThread = id; setTab('agent'); }

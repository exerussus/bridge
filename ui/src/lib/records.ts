// Записи пространств: виды, переходы статусов (те же правила, что у узла — TypeRules), поля.

export const TYPE_TITLE: Record<string, string> = { task: 'задача', decision: 'решение', note: 'заметка', status: 'статус', command: 'команда', dto: 'данные (dto)' };
export const REC_TYPES = new Set(['task', 'decision', 'note', 'status', 'command', 'dto']);
export const BUILTIN = ['title', 'status', 'assignee', 'body', 'ref', 'priority', 'due', 'labels', 'deleted', 'runner', 'args', 'result'];
export const FIELD: Record<string, string> = { assignee: 'Исполнитель', body: 'Описание', ref: 'Задание', runner: 'Исполнитель команды', args: 'Параметры', result: 'Результат', priority: 'Приоритет', due: 'Срок', text: 'Текст', labels: 'Метки', agent: 'Решает агент', agentThread: 'Нить агента' };
export const OPEN = ['new', 'accepted', 'in_progress', 'review', 'pending', 'proposed', 'running'];

const MOVES: Record<string, [string, string, string][]> = {
  task: [['new', 'accepted', 'a'], ['new', 'rejected', 'a'], ['accepted', 'in_progress', 'a'], ['new', 'in_progress', 'a'], ['in_progress', 'review', 'a'], ['review', 'done', 'c'], ['review', 'in_progress', 'c'],
    ['new', 'cancelled', 'c'], ['accepted', 'cancelled', 'c'], ['in_progress', 'cancelled', 'c'], ['review', 'cancelled', 'c'], ['done', 'new', 'c'], ['rejected', 'new', 'c'], ['cancelled', 'new', 'c']],
  command: [['pending', 'cancelled', 'c'], ['accepted', 'cancelled', 'c']],
  decision: [['proposed', 'accepted', 'o'], ['proposed', 'rejected', 'o'], ['accepted', 'proposed', 'c'], ['rejected', 'proposed', 'c']],
};
const MOVE_TEXT: Record<string, string> = { accepted: 'Принять', rejected: 'Отказаться', in_progress: 'Начать', review: 'Сдать на проверку', done: 'Закрыть — готово', cancelled: 'Отменить', new: 'Открыть заново' };

export function allowedMoves(s: any, me: string): [string, string][] {
  const creator = s.creator === me, assignee = (s.fields.assignee || '') === me;
  return (MOVES[s.type] || [])
    .filter(([f, , who]) => f === s.status && (who === 'a' ? assignee : who === 'c' ? creator : !creator))
    .map(([, to]) => to)
    .map(to => (s.type === 'task' && s.status === 'review' && to === 'in_progress' ? ['in_progress', 'Вернуть на доработку'] : [to, s.type === 'decision' && to === 'accepted' ? 'Согласовать' : MOVE_TEXT[to] || to]) as [string, string]);
}

/** Поля «ключ — значение»: строка-число становится числом, true/false — флагом. */
export type KV = { k: string; v: string };
export function kvRead(rows: KV[]): Record<string, any> {
  const o: Record<string, any> = {};
  for (const r of rows) {
    const k = r.k.trim(), v = r.v;
    if (!k) continue;
    o[k] = v.trim() !== '' && !isNaN(+v) ? +v : v === 'true' ? true : v === 'false' ? false : v;
  }
  return o;
}

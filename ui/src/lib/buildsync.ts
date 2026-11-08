// (3.22.1) Сборки: согласование панели с мостом.
// gen — номер последнего сохранения заготовок: ответ GET builds, начатый раньше, уже устарел (иначе удалённая
// заготовка воскресала, а снятый соавтор возвращался). dirty — заготовки с несохранёнными правками: команду
// (gid/owner/authors) с моста в них не подмешиваем. deleted — недавно удалённые: не подмешивать обратно.
export const BS = { gen: 0, edits: new Map<string, number>(), deleted: new Map<string, number>() };

let seq = 0;
/** Отметить правку заготовки (до сохранения). */
export function touched(id: string) { BS.edits.set(id, ++seq); }
/** Начать сохранение: вернёт снимок правок — передать в saved() после удачного ответа. */
export function saving(): Map<string, number> { BS.gen++; return new Map(BS.edits); }
/** Сохранение прошло: снять отметки, если после него правок не было. */
export function saved(snap: Map<string, number>) { for (const [id, s] of snap) if (BS.edits.get(id) === s) BS.edits.delete(id); }
export function isDirty(id: string) { return BS.edits.has(id); }
export function markDeleted(id: string) { BS.deleted.set(id, Date.now()); BS.edits.delete(id); }
export function recentlyDeleted(id: string) {
  const t = BS.deleted.get(id); if (t === undefined) return false;
  if (Date.now() - t > 60000) { BS.deleted.delete(id); return false; }
  return true;
}

export const eqIc = (a?: string, b?: string) => !!a && !!b && a.toLowerCase() === b.toLowerCase();

/** Ключ согласия на запуск присланной сборки: у общей — ещё GUID и кто выпустил версию (другой отправитель — спросить заново). */
export function runKey(r: any): string {
  const base = r.peer + '/' + r.name + '|' + r.exe;
  return r.gid ? base + '|' + r.gid + '/' + (r.by || r.peer).toLowerCase() : base;
}

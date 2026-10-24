<script lang="ts">
  import { closeModal, toast, ui, peerTitle } from '../lib/state.svelte';
  import { api, act } from '../lib/api';
  import { TYPE_TITLE, BUILTIN, kvRead, type KV } from '../lib/records';
  import KvRows from '../components/KvRows.svelte';
  let { share, id, onsaved }: { share: string; id: string; onsaved: () => void } = $props();
  let st = $state<any>(null);
  let title = $state('');
  let body = $state('');
  let to = $state('');
  let prio = $state('');
  let due = $state('');
  let kv = $state<KV[]>([]);
  let extra: [string, any][] = [];
  const self = ui.ov.node.node;
  const sh = ui.ov.shares.find((x: any) => x.id === share) || { peers: [] };
  const who = [self, ...sh.peers.filter((p: string) => !p.startsWith('@'))];
  (async () => {
    try { st = await api('GET', 'records/' + encodeURIComponent(id) + '?share=' + encodeURIComponent(share)); } catch (e: any) { toast(e.message, 'bad'); closeModal(); return; }
    const f = st.fields || {};
    title = f.title || ''; body = typeof f.body === 'string' ? f.body : ''; to = f.assignee || self; prio = f.priority || ''; due = f.due || '';
    extra = Object.entries(f).filter(([k, v]) => !BUILTIN.includes(k) && v !== null && typeof v !== 'object');
    kv = extra.map(([k, v]) => ({ k, v: String(v) }));
  })();
  async function save() {
    const f = st.fields || {};
    const next: Record<string, any> = kvRead(kv);
    next.title = title.trim(); next.body = body;
    if (!next.title) { toast('Нужен заголовок', 'bad'); return; }
    if (st.type === 'task') { next.assignee = to; next.priority = prio; next.due = due; }
    const ch: Record<string, any> = {};
    for (const [k, v] of Object.entries(next)) { const old = f[k] ?? ''; if (String(old) !== String(v ?? '')) ch[k] = v === '' ? null : v; }
    for (const [k] of extra) if (!(k in next)) ch[k] = null;
    if (!Object.keys(ch).length) { closeModal(); return; }
    try { await act({ action: 'set', share, record: id, fields: ch, expectVersion: st.version, actor: 'панель' }); closeModal(); toast('Сохранено', 'ok'); onsaved(); } catch (e: any) { toast(e.message, 'bad'); }
  }
</script>

{#if st}
  <h3>Изменить · {st.fields.title || id} <span class="tag">{TYPE_TITLE[st.type] || st.type}</span></h3>
  <label class="f">Заголовок<input bind:value={title} /></label>
  <label class="f">Описание<textarea bind:value={body}></textarea></label>
  {#if st.type === 'task'}
    <div class="row" style="gap:10px">
      <label class="f" style="flex:1">Исполнитель<select bind:value={to}>{#each who as w}<option value={w}>{w === self ? 'я (' + w + ')' : peerTitle(w)}</option>{/each}</select></label>
      <label class="f" style="width:150px">Приоритет<select bind:value={prio}><option value="">—</option>{#each ['низкий', 'обычный', 'высокий', 'срочно'] as p}<option>{p}</option>{/each}</select></label>
      <label class="f" style="width:170px">Срок<input type="date" bind:value={due} /></label>
    </div>
  {/if}
  <div class="f">Поля<div><KvRows bind:rows={kv} /></div></div>
  <div class="small muted">Правка уходит всем участникам пространства и остаётся в истории записи (её можно откатить).</div>
  <div class="row" style="margin-top:12px"><div class="grow"></div><button class="b" onclick={closeModal}>Отмена</button><button class="b pri" onclick={save}>Сохранить</button></div>
{:else}
  <div class="small muted">Загружаю…</div>
{/if}

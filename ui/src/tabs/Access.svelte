<script lang="ts">
  import { ui, toast, load, titleOf, openModal } from '../lib/state.svelte';
  import { api } from '../lib/api';
  import Gi from '../components/Gi.svelte';
  import GroupEdit from '../modals/GroupEdit.svelte';
  let A = $state<any>(null);
  let err = $state('');
  async function refresh() { try { A = await api('GET', 'access'); err = ''; } catch (e: any) { err = e.message; } }
  $effect(() => { void ui.ov; refresh(); });
  const ACC: Record<string, [string, string]> = { write: ['запись', 'w'], read: ['чтение', ''] };
  async function cycle(fid: string, peer: string) {
    const f = (await api('GET', 'folders')).own.find((x: any) => x.id === fid);
    if (!f) return;
    const cur = f.access[peer], next = !cur ? 'read' : cur === 'read' ? 'write' : null;
    const access = { ...f.access };
    if (next) access[peer] = next; else delete access[peer];
    try { await api('POST', 'folders', { id: f.id, title: f.title, path: f.path, access }); toast('Доступ изменён', 'ok'); refresh(); } catch (e: any) { toast(e.message, 'bad'); }
  }
  async function delGroup(g: string) {
    if (!confirm('Удалить группу «' + g + '»? Доступ, выданный через неё, закроется.')) return;
    try { await api('POST', 'groups/remove?id=' + encodeURIComponent(g)); await load(); refresh(); } catch (e: any) { toast(e.message, 'bad'); }
  }
  const src = (c: any) => (c.direct ? 'напрямую' : c.via.length ? 'через ' + c.via.map((v: string) => v.split(':')[0]).join(', ') : c.all ? 'всем соседям' : '');
</script>

<div class="grid">
  <div class="card"><h3>Группы</h3>
    <div class="small muted" style="margin-bottom:8px">Соберите соседей в группы — «Семья», «Коллеги», — и давайте доступ группе целиком: новый член группы сразу видит всё, что видит группа, убрали из группы — доступ закрыт.</div>
    {#if A && Object.keys(A.groups || {}).length}
      <table><tbody>{#each Object.entries(A.groups) as [g, v]}
        {@const gv = v as any}
        <tr><td><b>{gv.title || g}</b><div class="small muted mono">@{g}</div></td>
          <td>{#each gv.members as m}<span class="chip">{titleOf(m)}</span>{:else}<span class="small muted">пусто</span>{/each}</td>
          <td style="white-space:nowrap"><button class="b sm" onclick={() => openModal(GroupEdit, { id: g, onsaved: refresh })}>Изменить</button> <button class="b sm danger" onclick={() => delGroup(g)}><Gi n="trash" /></button></td></tr>
      {/each}</tbody></table>
    {:else}<div class="empty">Групп пока нет</div>{/if}
    <div class="row" style="margin-top:10px"><button class="b pri" onclick={() => openModal(GroupEdit, { id: null, onsaved: refresh })}>＋ Группа</button></div>
  </div>
  <div class="card"><h3>Как читать таблицу</h3>
    <div class="small">Строки — соседи, столбцы — ваши общие папки и пространства. <span class="chip w">запись</span> — может класть, править, удалять; <span class="chip">чтение</span> — только видит; <b>—</b> — не видит. В скобках — откуда право: напрямую или через группу. Щёлкните клетку папки, чтобы изменить право прямо здесь.</div>
  </div>
</div>
<div class="card" style="margin-top:14px"><h3>Кто что видит</h3>
  <div style="overflow:auto">
    {#if err}<div class="pill bad">{err}</div>
    {:else if A}
      {#if !A.rows.length}<div class="empty">Соседей пока нет — подключите во вкладке «Соседи»</div>
      {:else if !(A.folders.length + A.shares.length)}<div class="empty">Вы пока ничем не поделились. Создайте папку во вкладке «Общие папки» — здесь появится, кто её видит.</div>
      {:else}
        <table class="matrix">
          <thead><tr><th>Сосед</th>{#each A.folders as f}<th><Gi n="folder" /> {f.title || f.id}</th>{/each}{#each A.shares as s}<th><Gi n="layers" /> {s.title || s.id}</th>{/each}</tr></thead>
          <tbody>
            {#each A.rows as r (r.peer)}
              <tr><td><b>{r.title || r.peer}</b>{#if r.groups.length}<div class="small muted">{r.groups.map((g: string) => '@' + g).join(' ')}</div>{/if}</td>
                {#each A.folders as f}
                  {@const c = r.folders[f.id]}
                  <td><span class="chip {c.access ? ACC[c.access][1] : ''}" style="cursor:pointer" title="щёлкните: нет → чтение → запись" role="button" tabindex="0" onclick={() => cycle(f.id, r.peer)} onkeydown={() => {}}>{c.access ? ACC[c.access][0] : '—'}</span>
                    {#if src(c)}<div class="small muted">{src(c)}</div>{/if}</td>
                {/each}
                {#each A.shares as s}<td>{#if r.shares[s.id]}<span class="chip w">участник</span>{:else}—{/if}</td>{/each}
              </tr>
            {/each}
          </tbody>
        </table>
      {/if}
    {/if}
  </div>
</div>

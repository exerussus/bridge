<script lang="ts">
  import { ui, peerTitle, openModal } from '../lib/state.svelte';
  import { api } from '../lib/api';
  import { ago, size } from '../lib/util';
  import { shareDel } from '../lib/shares';
  import Gi from '../components/Gi.svelte';
  import ShareEdit from '../modals/ShareEdit.svelte';

  let rejected = $state<Record<string, any[]>>({});
  let offers = $state<any[]>([]);
  let files = $state<Record<string, any[]>>({});
  let fq = $state<Record<string, string>>({});
  async function refresh() {
    const rj: Record<string, any[]> = {};
    for (const s of ui.ov.shares) { try { rj[s.id] = await api('GET', 'rejected?share=' + encodeURIComponent(s.id)); } catch { rj[s.id] = []; } }
    rejected = rj;
    try { offers = (await api('GET', 'shares/offers')).offers || []; } catch {}
  }
  $effect(() => { void ui.ov; refresh(); });
  async function loadFiles(id: string, e: Event) {
    if (!(e.target as HTMLDetailsElement).open || files[id]) return;
    try { files[id] = (await api('GET', 'files?share=' + encodeURIComponent(id))).files; } catch { files[id] = []; }
  }
  function join(x: any) { openModal(ShareEdit, { id: null, preset: { id: x.id, title: x.id, peers: [x.peer], accept: [{ pattern: '**', from: [x.peer] }] } }); }
</script>

<div class="row" style="margin-bottom:12px"><div class="small muted grow">Пространство — общая рабочая папка проекта с правилами: кто что принимает, плюс записи (задачи, решения, заметки). Пространство с одним id должно быть у каждого участника: создали здесь — соседи увидят предложение и подключат у себя одной кнопкой.</div>
  <button class="b pri" onclick={() => openModal(ShareEdit, { id: null })}>＋ Пространство</button></div>
{#if offers.length}
  <div class="card" style="margin-bottom:14px;border-color:rgba(56,225,240,.4)"><h3>Соседи открыли вам пространства</h3>
    {#each offers as x (x.id + x.peer)}
      <div class="row" style="margin-top:6px"><b>{x.id}</b><span class="small muted">у {peerTitle(x.peer)}</span><div class="grow"></div><button class="b sm pri" onclick={() => join(x)}>Подключить</button></div>
    {/each}
  </div>
{/if}
{#each ui.ov.shares as s (s.id)}
  {@const rej = rejected[s.id] || []}
  <div class="card" style="margin-bottom:14px">
    <div class="row"><h3 style="margin:0;font-size:15px;text-transform:none;color:var(--text)">{s.title || s.id} <span class="tag">{s.id}</span></h3><div class="grow"></div>
      <button class="b sm" data-open="share" data-share={s.id}><Gi n="folder" /> Папка</button>
      <button class="b sm" onclick={() => openModal(ShareEdit, { id: s.id })}>✎ Изменить</button>
      <button class="b sm danger" onclick={() => shareDel(s.id)}>Убрать</button></div>
    <div class="kv small" style="margin-top:8px">
      <div>Папка</div><div class="mono">{s.folder}</div>
      <div>Соседи</div><div>{s.peers.map(peerTitle).join(', ')}</div>
      <div>Файлов</div><div>{s.files} · {size(s.bytes)}</div>
      <div>Записей</div><div>{s.records} (открытых {s.open})</div>
      <div>Последний обмен</div><div>{ago(s.lastRound)}</div>
      <div>Принимаю</div><div>{#each s.accept as a, i}{#if i}<br />{/if}<span class="tag">{a.pattern}</span> ← {a.from.join(', ')}{:else}<span class="muted">ничего (только раздаю)</span>{/each}</div>
      <div>Схемы DTO</div><div>{#each s.schemas as x}<span class="tag">{x}</span> {:else}<span class="muted">нет</span>{/each}</div>
      {#each Object.entries(s.reports) as [p, r]}<div>{peerTitle(p)}</div><div>{r}</div>{/each}
    </div>
    {#if s.conflicts.length}<div style="margin-top:8px"><b class="small">Конфликты файлов</b>{#each s.conflicts as c}<div class="small mono">{c}</div>{/each}</div>{/if}
    {#if s.schemaErrors.length}<div class="small" style="color:var(--bad)">Схемы: {s.schemaErrors.join('; ')}</div>{/if}
    {#if rej.length}
      <details style="margin-top:8px"><summary class="small">Отклонённые события ({rej.length})</summary>
        {#each rej.slice(-50) as r}<div class="small"><span class="tag">{r.event.eid}</span> {r.event.type}/{r.event.op} — {r.reason}</div>{/each}</details>
    {/if}
    <details style="margin-top:8px" ontoggle={e => loadFiles(s.id, e)}><summary class="small">Файлы пространства</summary>
      <input placeholder="поиск" style="margin:6px 0" bind:value={fq[s.id]} />
      <div class="filelist small mono">
        {#each (files[s.id] || []).filter(f => !fq[s.id] || f.path.toLowerCase().includes(fq[s.id].toLowerCase())).slice(0, 400) as f (f.path)}
          <div>{f.path} <span class="muted">{size(f.size)} · {ago(f.modified)}</span></div>
        {:else}<div class="muted">нет</div>{/each}
      </div>
    </details>
  </div>
{:else}
  <div class="empty">Пространств пока нет. «＋ Пространство» — создать: например «Проект» с папкой заданий и отчётов для двух-трёх ПК.</div>
{/each}

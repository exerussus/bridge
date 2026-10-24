<script lang="ts">
  import { ui, toast, peerTitle, openModal, me, onTab } from '../lib/state.svelte';
  import { api, act } from '../lib/api';
  import { ago, size, fullTime } from '../lib/util';
  import { TYPE_TITLE, REC_TYPES, FIELD, OPEN, allowedMoves } from '../lib/records';
  import Pill from '../components/Pill.svelte';
  import Gi from '../components/Gi.svelte';
  import RecordNew from '../modals/RecordNew.svelte';
  import RecordEdit from '../modals/RecordEdit.svelte';
  import AgentTask from '../modals/AgentTask.svelte';
  import { openThread } from '../lib/agenttask';
  import RecAsk from '../components/RecAsk.svelte';

  let type = $state('');
  let status = $state('');
  let q = $state('');
  let list = $state<any[]>([]);
  let types = $state<string[]>([...REC_TYPES]);
  let rec = $state<any>(null);
  let hist = $state<any[]>([]);
  let comment = $state('');
  let qInput: HTMLInputElement;

  $effect(() => {
    if (!ui.recShare || !ui.ov.shares.some((s: any) => s.id === ui.recShare)) ui.recShare = ui.ov.shares[0]?.id || '';
  });

  let seq = 0;
  export async function refresh() {
    const share = ui.recShare;
    if (!share) { list = []; return; }
    const p = new URLSearchParams({ share });
    if (type) p.set('type', type);
    if (status && !status.startsWith('@')) p.set('status', status);
    if (status === '@deleted') p.set('deleted', '1');
    if (q) p.set('q', q);
    const my = ++seq;
    let r: any[];
    try { r = await api('GET', 'records?' + p); } catch (e: any) { toast(e.message, 'bad'); return; }
    if (my !== seq) return;
    for (const x of r) REC_TYPES.add(x.type);
    types = [...REC_TYPES];
    const m = me();
    if (status === '@open') r = r.filter(x => OPEN.includes(x.status));
    if (status === '@mine') r = r.filter(x => (x.assignee === m && ['new', 'accepted', 'in_progress'].includes(x.status)) || (x.creator === m && x.status === 'review') || (x.type === 'decision' && x.creator !== m && x.status === 'proposed') || x.conflict);
    r.sort((a, b) => +new Date(b.updated) - +new Date(a.updated));
    list = r;
    if (ui.curRecord) show(ui.curRecord, true);
  }
  // фильтры и обновление обзора → перечитать
  $effect(() => { void ui.recShare; void type; void status; void ui.ov; refresh(); });
  let qt: any;
  function onQ() { clearTimeout(qt); qt = setTimeout(refresh, 250); }

  async function show(id: string, quiet = false) {
    ui.curRecord = id;
    const share = ui.recShare;
    try {
      const s = await api('GET', 'records/' + encodeURIComponent(id) + '?share=' + encodeURIComponent(share));
      const h = await api('GET', 'records/' + encodeURIComponent(id) + '/history?share=' + encodeURIComponent(share));
      if (ui.curRecord !== id) return;
      rec = s; hist = h;
    } catch (e: any) { if (!quiet) toast(e.message, 'bad'); }
  }
  const fields = $derived.by(() => {
    if (!rec) return [];
    const f: Record<string, any> = { ...rec.fields };
    for (const k of ['title', 'status', 'deleted']) delete f[k];
    return Object.entries(f).filter(([, v]) => v !== null && v !== '');
  });
  const moves = $derived(rec ? allowedMoves(rec, me()) : []);

  async function move(to: string) {
    let note: string | null = '';
    if (to === 'rejected' || (to === 'in_progress' && rec.status === 'review') || to === 'cancelled') { note = prompt('Пояснение:', ''); if (note === null) return; }
    try { await act({ action: 'transition', share: ui.recShare, record: rec.id, to, note, actor: 'панель' }); toast('Готово', 'ok'); refresh(); } catch (e: any) { toast(e.message, 'bad'); }
  }
  async function del(d: boolean) {
    if (d && !confirm('Удалить запись у всех участников пространства? Вернуть можно: фильтр статуса → «удалённые».')) return;
    try {
      await act({ action: d ? 'delete' : 'restore', share: ui.recShare, record: rec.id, actor: 'панель' });
      toast(d ? 'Удалено' : 'Возвращено', 'ok');
      if (d) { ui.curRecord = null; rec = null; }
      refresh();
    } catch (e: any) { toast(e.message, 'bad'); }
  }
  async function send() {
    const t = comment.trim(); if (!t) return;
    try { await act({ action: 'comment', share: ui.recShare, record: rec.id, text: t, actor: 'панель' }); comment = ''; show(rec.id); } catch (e: any) { toast(e.message, 'bad'); }
  }
  async function ack(level: string) {
    try { await act({ action: 'ack', share: ui.recShare, record: rec.id, level, actor: 'панель' }); toast('Отмечено'); show(rec.id); } catch (e: any) { toast(e.message, 'bad'); }
  }
  function created(id: string) { ui.curRecord = id; refresh(); }
  function edited() { show(rec.id); refresh(); }

  // «/» — поиск записей
  $effect(() => {
    const k = (e: KeyboardEvent) => { if (e.key === '/' && ui.tab === 'records' && !(e.target as HTMLElement).closest('input,textarea,select')) { e.preventDefault(); qInput?.focus(); } };
    addEventListener('keydown', k);
    return () => removeEventListener('keydown', k);
  });
  onTab(() => {});
</script>

<div class="split">
  <div class="card">
    <div class="row" style="margin-bottom:8px">
      <select bind:value={ui.recShare}>{#each ui.ov.shares as s (s.id)}<option>{s.id}</option>{/each}</select>
      <select bind:value={type}><option value="">все типы</option>{#each types as t}<option value={t}>{TYPE_TITLE[t] || t}</option>{/each}</select>
      <select bind:value={status}><option value="">любой статус</option><option value="@open">открытые</option><option value="@mine">ждут меня</option><option>new</option><option>accepted</option><option>in_progress</option><option>review</option><option>done</option><option>pending</option><option>proposed</option><option>rejected</option><option>failed</option><option value="@deleted">удалённые</option></select>
      <input bind:this={qInput} bind:value={q} oninput={onQ} placeholder="поиск (/)" style="flex:1;min-width:120px" />
      <button class="b pri" onclick={() => { const sh = ui.ov.shares.find((s: any) => s.id === ui.recShare); if (!sh) { toast('Сначала создайте пространство', 'bad'); return; } openModal(RecordNew, { share: ui.recShare, oncreated: created }); }}>＋ Создать</button>
    </div>
    <table>
      <thead><tr><th>Запись</th><th>Статус</th><th>Кому</th><th>Обновлена</th></tr></thead>
      <tbody>
        {#if !ui.recShare}<tr><td colspan="4" class="empty">Нет пространств</td></tr>{/if}
        {#each list as r (r.id)}
          <tr class="click" class:on={ui.curRecord === r.id} onclick={() => show(r.id)}>
            <td><b>{r.title || r.id}</b>{#if r.conflict} <span class="pill bad">конфликт</span>{/if}
              <div class="small muted"><span class="tag">{r.type}</span> {r.id} · v{r.version}{#if r.comments} · <Gi n="chat" />{r.comments}{/if}</div></td>
            <td><Pill s={r.status} /></td><td class="small">{r.assignee ? peerTitle(r.assignee) : ''}</td>
            <td class="small muted" title={fullTime(r.updated)}>{ago(r.updated)}</td>
          </tr>
        {:else}
          {#if ui.recShare}<tr><td colspan="4" class="empty">Записей нет</td></tr>{/if}
        {/each}
      </tbody>
    </table>
  </div>
  <div class="card">
    {#if !rec}<div class="empty">Выберите запись слева</div>
    {:else}
      {@const s = rec}
      <div class="row"><h3 style="margin:0;text-transform:none;font-size:16px;color:var(--text)">{s.fields.title || s.id}</h3><div class="grow"></div><Pill s={s.status} />
        {#if s.fields.deleted}<span class="pill warn">удалена</span><button class="b sm" onclick={() => del(false)}>↩ Вернуть</button>
        {:else}{#if s.type === 'task'}<button class="b sm pri" title="Поручить задачу Claude на соседнем ПК — статус задачи он поведёт сам" disabled={['done', 'cancelled', 'rejected'].includes(s.status)} onclick={() => openModal(AgentTask, { share: ui.recShare, rec: s, onstarted: () => show(s.id) })}>🤖 Агенту</button>{/if}
          <button class="b sm" onclick={() => openModal(RecordEdit, { share: ui.recShare, id: s.id, onsaved: edited })}>✎ Изменить</button>
          <button class="b sm danger" title="Удалить у всех участников пространства (можно вернуть: фильтр «удалённые»)" onclick={() => del(true)}><Gi n="trash" /> Удалить</button>{/if}
      </div>
      <div class="small muted" style="margin:4px 0 10px"><span class="tag">{s.type}</span> {s.id} · версия {s.version}{#if s.schema} · схема <span class="tag">{s.schema}</span>{/if} · создал {peerTitle(s.creator)}{s.creatorActor ? ' (' + s.creatorActor + ')' : ''} {ago(s.created)}</div>
      {#if s.fields.agent}
        <div class="banner" style="margin-bottom:10px"><span>🤖</span><div class="grow small">Решает агент на <b>{peerTitle(s.fields.agent)}</b> — статус задачи он ведёт сам, «готово» ставите вы.</div>{#if s.fields.agentThread}<button class="b sm" onclick={() => openThread(s.fields.agentThread)}>Ход работы</button>{/if}</div>
        {#if s.fields.agentThread}<RecAsk id={s.fields.agentThread} />{/if}
      {/if}
      {#if Object.keys(s.delivered || {}).length}
        <div class="row" style="margin-bottom:10px">{#each Object.entries(s.delivered) as [p, ok]}<span class="pill {ok ? 'ok' : 'warn'}">{peerTitle(p)}: {ok ? 'доставлено' : 'ещё нет'}</span> {/each}</div>
      {/if}
      {#if Object.keys(s.conflicts || {}).length}
        <div class="card" style="background:var(--bad-soft);border-color:transparent;margin-bottom:10px"><b>Встречные правки</b>
          {#each Object.entries(s.conflicts) as [k, v]}<div class="small"><span class="tag">{k}</span>: {#each (v as any[]) as x, i}{i ? ' · ' : ''}{x.author} → <span class="mono">{JSON.stringify(x.value)}</span>{/each}</div>{/each}
          <div class="small muted">Снимет следующая правка поля.</div></div>
      {/if}
      {#if s.schemaErrors}<div class="pill bad" style="margin-bottom:8px">не по схеме: {s.schemaErrors.join('; ')}</div>{/if}
      <div class="kv" style="margin-bottom:10px">
        {#each fields as [k, v] (k)}
          <div title={k}>{FIELD[k] || k}</div>
          <div>
            {#if k === 'assignee' && typeof v === 'string'}{peerTitle(v)}
            {:else if k === 'result' && v && typeof v === 'object'}{v.exitCode != null ? 'код ' + v.exitCode + ' · ' : ''}{v.message || ''}{#if v.log} · лог <span class="mono">{v.log}</span>{/if}
            {:else if typeof v === 'string'}{#if k === 'body'}<pre>{v}</pre>{:else}{v}{/if}
            {:else}<pre>{JSON.stringify(v, null, 1)}</pre>{/if}
          </div>
        {/each}
      </div>
      {#if s.attachments.length}
        <div style="margin-bottom:8px"><b class="small">Вложения</b>
          {#each s.attachments.filter((a: any) => !a.reverted) as a}<div class="small mono">📎 {a.path} <span class="muted">{size(a.size)}{a.title ? ' · ' + a.title : ''}</span></div>{/each}</div>
      {/if}
      {#if moves.length}
        <div class="row" style="margin:8px 0">{#each moves as [m, label]}<button class="b sm" class:pri={m === 'done' || m === 'accepted'} class:danger={m === 'rejected' || m === 'cancelled'} onclick={() => move(m)}>{label}</button>{/each}</div>
      {:else}
        <div class="small muted" style="margin:8px 0">{['done', 'rejected', 'cancelled', 'failed'].includes(s.status) ? 'Запись закрыта.' : 'Следующий шаг — за другой стороной.'}</div>
      {/if}
      <div><b class="small">Обсуждение</b>
        {#each s.comments.filter((c: any) => !c.reverted) as c}
          <div class="comment"><div class="small muted">{peerTitle(c.author)}{c.actor ? ' · ' + c.actor : ''} · {ago(c.time)}</div>{c.text}</div>
        {:else}<div class="small muted">Комментариев нет</div>{/each}
        <textarea bind:value={comment} placeholder="Комментарий… (Ctrl+Enter — отправить)" onkeydown={e => { if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) send(); }}></textarea>
        <div class="row" style="margin-top:6px"><div class="grow"></div>
          <button class="b sm" onclick={() => ack('seen')}><Gi n="eye" /> Видел</button><button class="b sm" onclick={() => ack('accepted')}>✔ Согласен</button><button class="b pri sm" onclick={send}>Отправить</button></div>
      </div>
      {#if s.acks.length}<div class="small muted" style="margin-top:6px">{s.acks.map((a: any) => peerTitle(a.author) + ': ' + a.level + (a.text ? ' «' + a.text + '»' : '') + ' (v' + a.version + ')').join(' · ')}</div>{/if}
      <details style="margin-top:12px"><summary class="small">История ({hist.length} событий)</summary>
        <table class="hist"><tbody>
          {#each hist as e}
            <tr><td>v{e.version}</td><td><span class="tag">{e.op}</span></td><td>{peerTitle(e.author)}{#if e.actor}<div class="small muted">{e.actor}</div>{/if}</td>
              <td><pre>{JSON.stringify(e.payload)}</pre>{#if e.note}<div class="small">{e.note}</div>{/if}{#if e.invalid}<div class="small" style="color:var(--bad)">не применено: {e.invalid}</div>{/if}</td>
              <td class="small muted" title={fullTime(e.time)}>{ago(e.time)}</td></tr>
          {/each}
        </tbody></table>
      </details>
    {/if}
  </div>
</div>
